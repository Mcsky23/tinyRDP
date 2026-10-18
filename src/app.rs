use crate::{
    credentials, login,
    model::{Device, Resolution, SharedFolder, validate_shared_folders},
    rdp,
    store::Store,
};
use anyhow::{Context, Result};
use dispatch2::DispatchQueue;
use objc2::{
    AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject, Sel},
    sel,
};
use objc2_app_kit::*;
use objc2_foundation::{
    MainThreadMarker, NSData, NSIndexSet, NSNotification, NSObject, NSObjectProtocol, NSPoint,
    NSRect, NSSize, NSString, ns_string,
};
use objc2_service_management::SMAppServiceStatus;
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::HashSet,
};
use uuid::Uuid;

mod folders;
use folders::FolderEditor;

#[cfg(debug_assertions)]
mod smoke;

const RESOLUTIONS: &[Resolution] = &[
    Resolution {
        width: 1280,
        height: 720,
    },
    Resolution {
        width: 1280,
        height: 800,
    },
    Resolution {
        width: 1366,
        height: 768,
    },
    Resolution {
        width: 1440,
        height: 900,
    },
    Resolution {
        width: 1600,
        height: 900,
    },
    Resolution {
        width: 1680,
        height: 1050,
    },
    Resolution {
        width: 1920,
        height: 1080,
    },
    Resolution {
        width: 1920,
        height: 1200,
    },
    Resolution {
        width: 2560,
        height: 1440,
    },
    Resolution {
        width: 2560,
        height: 1600,
    },
    Resolution {
        width: 3440,
        height: 1440,
    },
    Resolution {
        width: 3840,
        height: 2160,
    },
];

thread_local! {
    static DELEGATE: OnceCell<Retained<AppDelegate>> = const { OnceCell::new() };
}

struct State {
    store: RefCell<Store>,
    status_item: OnceCell<Retained<NSStatusItem>>,
    history_item: OnceCell<Retained<NSMenuItem>>,
    connect_menu: OnceCell<Retained<NSMenu>>,
    login_item: OnceCell<Retained<NSMenuItem>>,
    editor: OnceCell<Editor>,
    folder_editor: OnceCell<FolderEditor>,
    baseline: RefCell<Device>,
    switching: Cell<bool>,
    connecting: RefCell<HashSet<Uuid>>,
    sessions: RefCell<HashSet<Uuid>>,
}

struct Editor {
    window: Retained<NSWindow>,
    table: Retained<NSTableView>,
    heading: Retained<NSTextField>,
    name: Retained<NSTextField>,
    host: Retained<NSTextField>,
    port: Retained<NSTextField>,
    username: Retained<NSTextField>,
    domain: Retained<NSTextField>,
    password: Retained<NSSecureTextField>,
    remember: Retained<NSButton>,
    resolution: Retained<NSPopUpButton>,
    resolution_width: Retained<NSTextField>,
    resolution_height: Retained<NSTextField>,
    resolution_separator: Retained<NSTextField>,
    fullscreen: Retained<NSButton>,
    clipboard: Retained<NSButton>,
    invert_scroll: Retained<NSButton>,
    folders_button: Retained<NSButton>,
    shared_folders: RefCell<Vec<SharedFolder>>,
    remove: Retained<NSButton>,
    feedback: Retained<NSTextField>,
}

// SAFETY: The delegate is confined to the main thread. NSObject has no additional
// subclassing requirements, and all action signatures match their selectors.
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = State]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}
    unsafe impl NSControlTextEditingDelegate for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            self.setup_menu();
            #[cfg(debug_assertions)]
            if std::env::args().any(|arg| arg == "--smoke-test") {
                DispatchQueue::main().exec_async(|| {
                    DELEGATE.with(|cell| {
                        smoke::run(cell.get().unwrap()).expect("Native UI smoke test failed.");
                    });
                });
            }
            if std::env::args().any(|arg| arg == "--manage-devices") { self.show_editor(); }
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn terminate_after_last_window(&self, _application: &NSApplication) -> bool { false }

        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _application: &NSApplication) -> NSApplicationTerminateReply {
            if self.confirm_discard() { NSApplicationTerminateReply::TerminateNow }
            else { NSApplicationTerminateReply::TerminateCancel }
        }
    }

    unsafe impl NSMenuDelegate for AppDelegate {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, _menu: &NSMenu) { self.refresh_login(); }
    }

    unsafe impl NSWindowDelegate for AppDelegate {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _window: &NSWindow) -> bool { self.confirm_discard() }

        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            let id = self.ivars().baseline.borrow().id;
            self.load_device(Some(id));
        }
    }

    unsafe impl NSTableViewDataSource for AppDelegate {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, table: &NSTableView) -> isize {
            if self.is_folder_table(table) {
                return self.ivars().folder_editor.get().unwrap().draft.borrow().len() as isize;
            }
            self.ivars().store.borrow().config.devices.len() as isize
        }

        #[unsafe(method_id(tableView:objectValueForTableColumn:row:))]
        fn object_value(&self, table: &NSTableView, column: Option<&NSTableColumn>, row: isize) -> Option<Retained<AnyObject>> {
            if self.is_folder_table(table) {
                let folders = self.ivars().folder_editor.get().unwrap().draft.borrow();
                folders.get(row as usize).map(|folder| {
                    let value = if column.is_some_and(|column| column.identifier().to_string() == "shareName") {
                        folder.name.clone()
                    } else { folder.path.display().to_string() };
                    NSString::from_str(&value).into()
                })
            } else {
                let store = self.ivars().store.borrow();
                store.config.devices.get(row as usize).map(|d| NSString::from_str(&d.name).into())
            }
        }

        #[unsafe(method(tableView:setObjectValue:forTableColumn:row:))]
        fn set_object_value(&self, table: &NSTableView, value: Option<&AnyObject>, _column: Option<&NSTableColumn>, row: isize) {
            if self.is_folder_table(table) {
                if let Some(value) = value.and_then(|value| value.downcast_ref::<NSString>()) {
                    if let Err(error) = self.rename_shared_folder(row, &value.to_string()) {
                        self.folder_feedback(&error.to_string());
                        self.ivars().folder_editor.get().unwrap().table.reloadData();
                    }
                }
            }
        }
    }

    unsafe impl NSTableViewDelegate for AppDelegate {
        #[unsafe(method(selectionShouldChangeInTableView:))]
        fn selection_should_change(&self, table: &NSTableView) -> bool {
            self.is_folder_table(table) || self.ivars().switching.get() || self.confirm_discard()
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_changed(&self, notification: &NSNotification) {
            if notification.object().and_then(|object| object.downcast::<NSTableView>().ok()).is_some_and(|table| self.is_folder_table(&table)) {
                self.update_folder_selection();
                return;
            }
            if self.ivars().switching.get() { return; }
            if let Some(editor) = self.ivars().editor.get() {
                let row = editor.table.selectedRow();
                let id = self.ivars().store.borrow().config.devices.get(row as usize).map(|d| d.id);
                self.load_device(id);
            }
        }
    }

    impl AppDelegate {
        #[unsafe(method(manageDevices:))]
        fn manage_devices(&self, _sender: Option<&AnyObject>) { self.show_editor(); }

        #[unsafe(method(addDevice:))]
        fn add_device(&self, _sender: Option<&AnyObject>) {
            if self.confirm_discard() {
                self.select_device(None);
                if let Some(editor) = self.ivars().editor.get() {
                    editor.window.makeFirstResponder(Some(&editor.name));
                }
            }
        }

        #[unsafe(method(saveDevice:))]
        fn save_device_action(&self, _sender: Option<&AnyObject>) {
            if let Err(error) = self.save_device() { self.show_error("Couldn’t save device", &error); }
        }

        #[unsafe(method(resolutionChanged:))]
        fn resolution_changed(&self, _sender: Option<&AnyObject>) { self.update_resolution_controls(); }

        #[unsafe(method(manageFolders:))]
        fn manage_folders(&self, _sender: Option<&AnyObject>) {
            if let Err(error) = self.show_shared_folders() { self.show_error("Couldn’t update shared folders", &error); }
        }

        #[unsafe(method(addSharedFolder:))]
        fn add_shared_folder(&self, _sender: Option<&AnyObject>) {
            if let Err(error) = self.choose_shared_folders() { self.folder_feedback(&error.to_string()); }
        }

        #[unsafe(method(removeSharedFolder:))]
        fn remove_shared_folder(&self, _sender: Option<&AnyObject>) { self.remove_selected_shared_folder(); }

        #[unsafe(method(removeDevice:))]
        fn remove_device_action(&self, _sender: Option<&AnyObject>) {
            if let Err(error) = self.remove_device() { self.show_error("Couldn’t remove device", &error); }
        }

        #[unsafe(method(toggleLogin:))]
        fn toggle_login(&self, _sender: Option<&AnyObject>) {
            let status = login::status();
            if status == SMAppServiceStatus::RequiresApproval {
                login::open_settings();
                return;
            }
            if let Err(error) = login::set_enabled(status != SMAppServiceStatus::Enabled) {
                self.show_error("Couldn’t update start on boot", &error);
            } else if login::status() == SMAppServiceStatus::RequiresApproval {
                alert(self.mtm(), "Allow tinyRDP at login", "Enable tinyRDP in System Settings → General → Login Items & Extensions.", &["Open Settings"]);
                login::open_settings();
            }
            self.refresh_login();
        }

        #[unsafe(method(connectDevice:))]
        fn connect_device(&self, sender: &NSMenuItem) {
            let id = sender.representedObject()
                .and_then(|o| o.downcast::<NSString>().ok())
                .and_then(|s| Uuid::parse_str(&s.to_string()).ok());
            let device = id.and_then(|id| self.ivars().store.borrow().config.devices.iter().find(|d| d.id == id).cloned());
            if let Some(device) = device {
                if let Err(error) = self.connect(device) { self.show_error("Couldn’t connect", &error); }
            }
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker, store: Store) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(State {
            store: RefCell::new(store),
            status_item: OnceCell::new(),
            history_item: OnceCell::new(),
            connect_menu: OnceCell::new(),
            login_item: OnceCell::new(),
            editor: OnceCell::new(),
            folder_editor: OnceCell::new(),
            baseline: RefCell::new(Device::default()),
            switching: Cell::new(false),
            connecting: RefCell::new(HashSet::new()),
            sessions: RefCell::new(HashSet::new()),
        });
        // SAFETY: NSObject's initializer takes no arguments and returns this class.
        unsafe { msg_send![super(this), init] }
    }

    fn menu_item(&self, title: &str, action: Option<Sel>, key: &str) -> Retained<NSMenuItem> {
        // SAFETY: Every supplied selector is an action implemented by this delegate.
        unsafe {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm()),
                &NSString::from_str(title),
                action,
                &NSString::from_str(key),
            );
            if action.is_some() {
                item.setTarget(Some(self));
            }
            item
        }
    }

    fn setup_menu(&self) {
        let mtm = self.mtm();
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        let history = self.menu_item("No history", Some(sel!(connectDevice:)), "");
        history.setEnabled(false);
        menu.addItem(&history);
        self.ivars().history_item.set(history).ok();
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let connect = self.menu_item("Connect", None, "");
        let devices = NSMenu::new(mtm);
        devices.setAutoenablesItems(false);
        connect.setSubmenu(Some(&devices));
        self.ivars().connect_menu.set(devices).ok();
        menu.addItem(&connect);

        let settings = self.menu_item("Settings", None, "");
        let settings_menu = NSMenu::new(mtm);
        settings_menu.setAutoenablesItems(false);
        // SAFETY: This delegate implements NSMenuDelegate and outlives the menu.
        settings_menu.setDelegate(Some(ProtocolObject::from_ref(self)));
        settings_menu.addItem(&self.menu_item("Manage devices…", Some(sel!(manageDevices:)), ","));
        let login_item = self.menu_item("Start on boot", Some(sel!(toggleLogin:)), "");
        settings_menu.addItem(&login_item);
        self.ivars().login_item.set(login_item).ok();
        settings.setSubmenu(Some(&settings_menu));
        menu.addItem(&settings);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        // SAFETY: terminate: is NSApplication's standard quit action.
        let exit = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                ns_string!("Exit"),
                Some(sel!(terminate:)),
                ns_string!("q"),
            )
        };
        menu.addItem(&exit);

        let status = NSStatusBar::systemStatusBar().statusItemWithLength(28.0);
        if let Some(button) = status.button(mtm) {
            let data = NSData::with_bytes(include_bytes!("../resources/StatusIcon.pdf"));
            if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
                image.setSize(NSSize::new(18.0, 18.0));
                image.setTemplate(true);
                button.setImage(Some(&image));
            } else {
                button.setTitle(ns_string!("tR"));
            }
            button.setToolTip(Some(ns_string!("tinyRDP — Remote Desktop")));
            button.setAccessibilityLabel(Some(ns_string!("tinyRDP")));
        }
        status.setMenu(Some(&menu));
        self.ivars().status_item.set(status).ok();
        self.refresh_connect();
        self.refresh_login();
        self.setup_edit_menu();
    }

    fn setup_edit_menu(&self) {
        let mtm = self.mtm();
        let main = NSMenu::new(mtm);
        let app_item = self.menu_item("tinyRDP", None, "");
        let app_menu = NSMenu::new(mtm);
        // SAFETY: These are standard responder chain actions.
        unsafe {
            app_menu.addItemWithTitle_action_keyEquivalent(
                ns_string!("Quit tinyRDP"),
                Some(sel!(terminate:)),
                ns_string!("q"),
            );
        }
        app_item.setSubmenu(Some(&app_menu));
        main.addItem(&app_item);
        let edit_item = self.menu_item("Edit", None, "");
        let edit_menu = NSMenu::new(mtm);
        for (title, action, key) in [
            ("Undo", sel!(undo:), "z"),
            ("Cut", sel!(cut:), "x"),
            ("Copy", sel!(copy:), "c"),
            ("Paste", sel!(paste:), "v"),
            ("Select All", sel!(selectAll:), "a"),
        ] {
            // SAFETY: Cocoa resolves these selectors through the responder chain.
            unsafe {
                edit_menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(title),
                    Some(action),
                    &NSString::from_str(key),
                );
            }
        }
        edit_item.setSubmenu(Some(&edit_menu));
        main.addItem(&edit_item);
        NSApplication::sharedApplication(mtm).setMainMenu(Some(&main));
    }

    fn refresh_connect(&self) {
        let Some(menu) = self.ivars().connect_menu.get() else {
            return;
        };
        menu.removeAllItems();
        let (devices, last_connected_device) = {
            let store = self.ivars().store.borrow();
            (
                store.config.devices.clone(),
                store.config.last_connected_device,
            )
        };
        if let Some(history) = self.ivars().history_item.get() {
            if let Some(device) = devices
                .iter()
                .find(|device| Some(device.id) == last_connected_device)
            {
                self.configure_device_item(history, device);
            } else {
                history.setTitle(ns_string!("No history"));
                history.setEnabled(false);
                history.setToolTip(None);
                // SAFETY: Clearing a represented object is valid for this action.
                unsafe {
                    history.setRepresentedObject(None);
                }
            }
        }
        if devices.is_empty() {
            let empty = self.menu_item("No saved devices", None, "");
            empty.setEnabled(false);
            menu.addItem(&empty);
            menu.addItem(&self.menu_item("Add a device…", Some(sel!(manageDevices:)), ""));
        } else {
            for device in devices {
                let item = self.menu_item("", Some(sel!(connectDevice:)), "");
                self.configure_device_item(&item, &device);
                menu.addItem(&item);
            }
        }
    }

    fn configure_device_item(&self, item: &NSMenuItem, device: &Device) {
        let running = self.ivars().sessions.borrow().contains(&device.id);
        let connecting = self.ivars().connecting.borrow().contains(&device.id);
        let title = if connecting {
            format!("{} (connecting…)", device.name)
        } else if running {
            format!("{} (session open)", device.name)
        } else {
            device.name.clone()
        };
        item.setTitle(&NSString::from_str(&title));
        // SAFETY: connectDevice: reads this represented object as an NSString.
        unsafe {
            item.setRepresentedObject(Some(&NSString::from_str(&device.id.to_string())));
        }
        item.setToolTip(Some(&NSString::from_str(&format!(
            "{} · {}",
            device.address(),
            device.username
        ))));
        item.setEnabled(!running && !connecting);
    }

    fn refresh_login(&self) {
        if let Some(item) = self.ivars().login_item.get() {
            let status = login::status();
            item.setState(if status == SMAppServiceStatus::Enabled {
                NSControlStateValueOn
            } else if status == SMAppServiceStatus::RequiresApproval {
                NSControlStateValueMixed
            } else {
                NSControlStateValueOff
            });
            item.setTitle(if status == SMAppServiceStatus::RequiresApproval {
                ns_string!("Start on boot — approval needed…")
            } else {
                ns_string!("Start on boot")
            });
            item.setToolTip(Some(ns_string!(
                "Launch tinyRDP when you sign in to macOS."
            )));
        }
    }

    fn show_editor(&self) {
        if self.ivars().editor.get().is_none() {
            let editor = self.build_editor();
            self.ivars().editor.set(editor).ok();
            let first = self
                .ivars()
                .store
                .borrow()
                .config
                .devices
                .first()
                .map(|d| d.id);
            self.select_device(first);
        }
        let editor = self.ivars().editor.get().unwrap();
        activate(self.mtm());
        editor.window.makeKeyAndOrderFront(None);
    }

    fn build_editor(&self) -> Editor {
        let mtm = self.mtm();
        // SAFETY: Auto-release on close is disabled immediately; Rust owns the window.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, 740.0, 600.0),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            window.setReleasedWhenClosed(false);
            window.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        window.setTitle(ns_string!("tinyRDP · Manage devices"));
        window.center();
        let content = window.contentView().unwrap();

        label(
            mtm,
            &content,
            "SAVED DEVICES",
            rect(24.0, 550.0, 190.0, 20.0),
            11.0,
            true,
        );
        let scroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(20.0, 76.0, 214.0, 466.0));
        scroll.setHasVerticalScroller(true);
        scroll.setBorderType(NSBorderType::BezelBorder);
        let table =
            NSTableView::initWithFrame(NSTableView::alloc(mtm), rect(0.0, 0.0, 210.0, 466.0));
        let column =
            NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("device"));
        column.setWidth(194.0);
        column.setEditable(false);
        table.addTableColumn(&column);
        table.setHeaderView(None);
        table.setRowHeight(30.0);
        table.setAllowsMultipleSelection(false);
        table.setAllowsEmptySelection(true);
        table.setAccessibilityLabel(Some(ns_string!("Saved devices")));
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(self)));
            table.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        scroll.setDocumentView(Some(&table));
        content.addSubview(&scroll);
        let add = self.button(
            "Add device",
            sel!(addDevice:),
            rect(20.0, 30.0, 112.0, 32.0),
        );
        content.addSubview(&add);
        let remove = self.button("Remove", sel!(removeDevice:), rect(138.0, 30.0, 96.0, 32.0));
        content.addSubview(&remove);

        let heading = label(
            mtm,
            &content,
            "New device",
            rect(270.0, 544.0, 440.0, 32.0),
            22.0,
            true,
        );
        let name = field(
            mtm,
            &content,
            "Name",
            "e.g. Work PC",
            rect(270.0, 480.0, 440.0, 26.0),
        );
        let host = field(
            mtm,
            &content,
            "Hostname or IP address",
            "192.168.1.10",
            rect(270.0, 420.0, 326.0, 26.0),
        );
        let port = field(
            mtm,
            &content,
            "Port",
            "3389",
            rect(610.0, 420.0, 100.0, 26.0),
        );
        let username = field(
            mtm,
            &content,
            "Username",
            "Remote account",
            rect(270.0, 360.0, 254.0, 26.0),
        );
        let domain = field(
            mtm,
            &content,
            "Domain (optional)",
            "WORKGROUP",
            rect(540.0, 360.0, 170.0, 26.0),
        );
        label(
            mtm,
            &content,
            "Password",
            rect(270.0, 327.0, 440.0, 20.0),
            12.0,
            false,
        );
        let password = NSSecureTextField::initWithFrame(
            NSSecureTextField::alloc(mtm),
            rect(270.0, 300.0, 440.0, 26.0),
        );
        password.setPlaceholderString(Some(ns_string!("Ask when connecting")));
        password.setAccessibilityLabel(Some(ns_string!("Password")));
        content.addSubview(&password);
        let remember = checkbox(
            mtm,
            "Remember password in Keychain",
            rect(268.0, 264.0, 442.0, 24.0),
        );
        content.addSubview(&remember);
        label(
            mtm,
            &content,
            "Leave blank to keep a saved password.",
            rect(271.0, 243.0, 439.0, 18.0),
            11.0,
            false,
        );
        label(
            mtm,
            &content,
            "Resolution",
            rect(270.0, 210.0, 440.0, 20.0),
            12.0,
            false,
        );
        let resolution = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(266.0, 178.0, 238.0, 30.0),
            false,
        );
        resolution.addItemWithTitle(ns_string!("Match current screen"));
        for size in RESOLUTIONS {
            resolution.addItemWithTitle(&NSString::from_str(&format!(
                "{} × {}",
                size.width, size.height
            )));
        }
        resolution.addItemWithTitle(ns_string!("Custom…"));
        resolution.setAccessibilityLabel(Some(ns_string!("Resolution")));
        resolution.setToolTip(Some(ns_string!(
            "Choose the remote desktop resolution for this device. Reconnect to apply changes."
        )));
        // SAFETY: The delegate implements the resolutionChanged: action and
        // outlives every control in this window.
        unsafe {
            resolution.setTarget(Some(self));
            resolution.setAction(Some(sel!(resolutionChanged:)));
        }
        content.addSubview(&resolution);
        let resolution_width =
            NSTextField::initWithFrame(NSTextField::alloc(mtm), rect(514.0, 180.0, 86.0, 26.0));
        resolution_width.setPlaceholderString(Some(ns_string!("Width")));
        resolution_width.setAccessibilityLabel(Some(ns_string!("Resolution width")));
        content.addSubview(&resolution_width);
        let resolution_separator = label(
            mtm,
            &content,
            "×",
            rect(604.0, 182.0, 18.0, 20.0),
            13.0,
            false,
        );
        let resolution_height =
            NSTextField::initWithFrame(NSTextField::alloc(mtm), rect(624.0, 180.0, 86.0, 26.0));
        resolution_height.setPlaceholderString(Some(ns_string!("Height")));
        resolution_height.setAccessibilityLabel(Some(ns_string!("Resolution height")));
        content.addSubview(&resolution_height);
        let fullscreen = checkbox(mtm, "Open in full screen", rect(268.0, 131.0, 442.0, 24.0));
        let clipboard = checkbox(mtm, "Share clipboard", rect(268.0, 103.0, 442.0, 24.0));
        let invert_scroll = checkbox(mtm, "Invert scrolling", rect(268.0, 75.0, 222.0, 24.0));
        invert_scroll.setToolTip(Some(ns_string!(
            "When off, scrolling follows macOS and your mouse utility. When on, reverse vertical and horizontal scrolling for this device. Save and reconnect to apply."
        )));
        content.addSubview(&fullscreen);
        content.addSubview(&clipboard);
        content.addSubview(&invert_scroll);
        let folders_button = self.button(
            "Shared folders…",
            sel!(manageFolders:),
            rect(500.0, 72.0, 212.0, 28.0),
        );
        folders_button.setToolTip(Some(ns_string!("Choose Mac folders to make available in this remote desktop. Save and reconnect to apply.")));
        content.addSubview(&folders_button);
        let feedback = label(
            mtm,
            &content,
            "",
            rect(270.0, 37.0, 280.0, 20.0),
            12.0,
            false,
        );
        let save = self.button(
            "Save device",
            sel!(saveDevice:),
            rect(576.0, 29.0, 140.0, 34.0),
        );
        save.setKeyEquivalent(ns_string!("\r"));
        content.addSubview(&save);
        window.setInitialFirstResponder(Some(&name));
        // SAFETY: Every view is retained by both Editor and the content view.
        unsafe {
            name.setNextKeyView(Some(&host));
            host.setNextKeyView(Some(&port));
            port.setNextKeyView(Some(&username));
            username.setNextKeyView(Some(&domain));
            domain.setNextKeyView(Some(&password));
            password.setNextKeyView(Some(&remember));
            remember.setNextKeyView(Some(&resolution));
            resolution.setNextKeyView(Some(&resolution_width));
            resolution_width.setNextKeyView(Some(&resolution_height));
            resolution_height.setNextKeyView(Some(&fullscreen));
            fullscreen.setNextKeyView(Some(&clipboard));
            clipboard.setNextKeyView(Some(&invert_scroll));
            invert_scroll.setNextKeyView(Some(&folders_button));
            folders_button.setNextKeyView(Some(&save));
        }
        Editor {
            window,
            table,
            heading,
            name,
            host,
            port,
            username,
            domain,
            password,
            remember,
            resolution,
            resolution_width,
            resolution_height,
            resolution_separator,
            fullscreen,
            clipboard,
            invert_scroll,
            folders_button,
            shared_folders: RefCell::new(Vec::new()),
            remove,
            feedback,
        }
    }

    fn button(&self, title: &str, action: Sel, frame: NSRect) -> Retained<NSButton> {
        // SAFETY: All button selectors are implemented by this delegate.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(self),
                Some(action),
                self.mtm(),
            )
        };
        button.setFrame(frame);
        button.setBezelStyle(NSBezelStyle::Push);
        button
    }

    fn load_device(&self, id: Option<Uuid>) {
        let Some(editor) = self.ivars().editor.get() else {
            return;
        };
        let saved = id.and_then(|id| {
            self.ivars()
                .store
                .borrow()
                .config
                .devices
                .iter()
                .find(|d| d.id == id)
                .cloned()
        });
        let is_saved = saved.is_some();
        let device = saved.unwrap_or_default();
        editor.heading.setStringValue(if is_saved {
            ns_string!("Edit device")
        } else {
            ns_string!("New device")
        });
        editor
            .name
            .setStringValue(&NSString::from_str(&device.name));
        editor
            .host
            .setStringValue(&NSString::from_str(&device.host));
        editor
            .port
            .setStringValue(&NSString::from_str(&device.port.to_string()));
        editor
            .username
            .setStringValue(&NSString::from_str(&device.username));
        editor
            .domain
            .setStringValue(&NSString::from_str(&device.domain));
        editor.password.setStringValue(ns_string!(""));
        editor.remember.setState(state(device.remember_password));
        let resolution_index = match device.resolution {
            None => 0,
            Some(size) => RESOLUTIONS
                .iter()
                .position(|preset| *preset == size)
                .map_or(RESOLUTIONS.len() + 1, |index| index + 1),
        };
        editor
            .resolution
            .selectItemAtIndex(resolution_index as isize);
        let (width, height) = device
            .resolution
            .map(|size| (size.width, size.height))
            .unwrap_or_else(|| self.desktop_size().unwrap_or((1920, 1080)));
        editor
            .resolution_width
            .setStringValue(&NSString::from_str(&width.to_string()));
        editor
            .resolution_height
            .setStringValue(&NSString::from_str(&height.to_string()));
        self.update_resolution_controls();
        editor.fullscreen.setState(state(device.fullscreen));
        editor.clipboard.setState(state(device.clipboard));
        editor.invert_scroll.setState(state(device.invert_scroll));
        editor.shared_folders.replace(device.shared_folders.clone());
        self.update_shared_folders_button();
        editor.remove.setEnabled(is_saved);
        editor.feedback.setStringValue(ns_string!(""));
        self.ivars().baseline.replace(device);
    }

    fn select_device(&self, id: Option<Uuid>) {
        let Some(editor) = self.ivars().editor.get() else {
            return;
        };
        self.ivars().switching.set(true);
        editor.table.reloadData();
        let row = id.and_then(|id| {
            self.ivars()
                .store
                .borrow()
                .config
                .devices
                .iter()
                .position(|d| d.id == id)
        });
        if let Some(row) = row {
            editor
                .table
                .selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row), false);
            editor.table.scrollRowToVisible(row as isize);
        } else {
            // SAFETY: No sender is required for this standard table action.
            unsafe {
                editor.table.deselectAll(None);
            }
        }
        self.ivars().switching.set(false);
        self.load_device(id);
    }

    fn update_resolution_controls(&self) {
        if let Some(editor) = self.ivars().editor.get() {
            let custom =
                editor.resolution.indexOfSelectedItem() == (RESOLUTIONS.len() + 1) as isize;
            editor.resolution_width.setHidden(!custom);
            editor.resolution_height.setHidden(!custom);
            editor.resolution_separator.setHidden(!custom);
        }
    }

    fn draft(&self) -> Result<Device> {
        let editor = self
            .ivars()
            .editor
            .get()
            .context("The device editor is not open.")?;
        let selection = editor.resolution.indexOfSelectedItem();
        let resolution = if selection == 0 {
            None
        } else if let Some(preset) = RESOLUTIONS.get((selection - 1) as usize) {
            Some(*preset)
        } else {
            Some(Resolution {
                width: editor
                    .resolution_width
                    .stringValue()
                    .to_string()
                    .trim()
                    .parse()
                    .context("Enter a resolution width between 200 and 8192.")?,
                height: editor
                    .resolution_height
                    .stringValue()
                    .to_string()
                    .trim()
                    .parse()
                    .context("Enter a resolution height between 200 and 8192.")?,
            })
        };
        Ok(Device {
            id: self.ivars().baseline.borrow().id,
            name: editor.name.stringValue().to_string().trim().into(),
            host: editor.host.stringValue().to_string().trim().into(),
            port: editor
                .port
                .stringValue()
                .to_string()
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .context("Enter a port between 1 and 65535.")?,
            username: editor.username.stringValue().to_string().trim().into(),
            domain: editor.domain.stringValue().to_string().trim().into(),
            remember_password: editor.remember.state() == NSControlStateValueOn,
            fullscreen: editor.fullscreen.state() == NSControlStateValueOn,
            clipboard: editor.clipboard.state() == NSControlStateValueOn,
            invert_scroll: editor.invert_scroll.state() == NSControlStateValueOn,
            resolution,
            shared_folders: editor.shared_folders.borrow().clone(),
        })
    }

    fn confirm_discard(&self) -> bool {
        let Some(editor) = self.ivars().editor.get() else {
            return true;
        };
        let dirty = self
            .draft()
            .map_or(true, |d| d != *self.ivars().baseline.borrow())
            || !editor.password.stringValue().is_empty();
        !dirty
            || alert(
                self.mtm(),
                "Discard unsaved changes?",
                "Your changes to this device haven’t been saved.",
                &["Keep Editing", "Discard Changes"],
            ) == NSAlertSecondButtonReturn
    }

    fn save_device(&self) -> Result<()> {
        let device = self.draft()?;
        device.validate()?;
        let editor = self.ivars().editor.get().unwrap();
        let password = editor.password.stringValue().to_string();
        anyhow::ensure!(
            !password.contains(['\r', '\n', '\0']),
            "Passwords cannot contain line breaks or NUL characters."
        );
        let mut devices = self.ivars().store.borrow().config.devices.clone();
        if device.remember_password {
            if !password.is_empty() {
                credentials::set(device.id, &password)?;
            }
        } else if devices
            .iter()
            .any(|d| d.id == device.id && d.remember_password)
        {
            credentials::remove(device.id)?;
        }
        if let Some(existing) = devices.iter_mut().find(|d| d.id == device.id) {
            *existing = device.clone();
        } else {
            devices.push(device.clone());
        }
        devices.sort_by_key(|d| d.name.to_lowercase());
        self.ivars().store.borrow_mut().commit(devices)?;
        self.select_device(Some(device.id));
        self.refresh_connect();
        editor.feedback.setStringValue(ns_string!("Device saved"));
        Ok(())
    }

    fn remove_device(&self) -> Result<()> {
        let id = self.ivars().baseline.borrow().id;
        let device = self
            .ivars()
            .store
            .borrow()
            .config
            .devices
            .iter()
            .find(|d| d.id == id)
            .cloned();
        let Some(device) = device else {
            return Ok(());
        };
        if alert(
            self.mtm(),
            "Remove device?",
            &format!("Remove “{}” and its saved password?", device.name),
            &["Cancel", "Remove"],
        ) != NSAlertSecondButtonReturn
        {
            return Ok(());
        }
        self.delete_saved_device(id)
    }

    fn delete_saved_device(&self, id: Uuid) -> Result<()> {
        if self
            .ivars()
            .store
            .borrow()
            .config
            .devices
            .iter()
            .any(|d| d.id == id && d.remember_password)
        {
            credentials::remove(id)?;
        }
        let mut devices = self.ivars().store.borrow().config.devices.clone();
        devices.retain(|d| d.id != id);
        let next = devices.first().map(|d| d.id);
        self.ivars().store.borrow_mut().commit(devices)?;
        self.select_device(next);
        self.refresh_connect();
        Ok(())
    }

    fn desktop_size(&self) -> Result<(u32, u32)> {
        // A menu bar app may have no key window. Pick the screen under the
        // Connect menu's pointer, including screens with negative origins.
        let mouse = NSEvent::mouseLocation();
        let screen = NSScreen::screens(self.mtm())
            .iter()
            .find(|screen| {
                let frame = screen.frame();
                mouse.x >= frame.origin.x
                    && mouse.x < frame.origin.x + frame.size.width
                    && mouse.y >= frame.origin.y
                    && mouse.y < frame.origin.y + frame.size.height
            })
            .or_else(|| NSScreen::mainScreen(self.mtm()))
            .context("Could not determine the current screen's resolution.")?;
        // Use macOS's scaled (Looks like) dimensions so Retina backing pixels
        // don't make the remote UI smaller than the local desktop.
        let size = screen.frame().size;
        Ok((size.width.round() as u32, size.height.round() as u32))
    }

    fn connect(&self, device: Device) -> Result<()> {
        if self.ivars().sessions.borrow().contains(&device.id)
            || self.ivars().connecting.borrow().contains(&device.id)
        {
            return Ok(());
        }
        let executable = rdp::backend()?;
        let desktop_size = match device.resolution {
            Some(size) => (size.width, size.height),
            None => self.desktop_size()?,
        };
        self.ivars().connecting.borrow_mut().insert(device.id);
        self.refresh_connect();
        if device.remember_password {
            activate(self.mtm());
            // Keep AppKit responsive while macOS waits for Touch ID/password.
            std::thread::spawn(move || {
                let saved = credentials::get_authenticated(device.id, &device.name);
                DispatchQueue::main().exec_async(move || {
                    DELEGATE.with(|cell| {
                        if let Some(delegate) = cell.get() {
                            if let Err(error) =
                                delegate.finish_connect(device, executable, desktop_size, saved)
                            {
                                delegate.show_error("Couldn’t connect", &error);
                            }
                        }
                    });
                });
            });
            Ok(())
        } else {
            self.finish_connect(
                device,
                executable,
                desktop_size,
                Ok(credentials::Lookup::Missing),
            )
        }
    }

    fn finish_connect(
        &self,
        device: Device,
        executable: std::path::PathBuf,
        desktop_size: (u32, u32),
        saved: Result<credentials::Lookup>,
    ) -> Result<()> {
        let id = device.id;
        let result =
            saved.and_then(|saved| self.start_connection(device, executable, desktop_size, saved));
        self.ivars().connecting.borrow_mut().remove(&id);
        self.refresh_connect();
        result
    }

    fn start_connection(
        &self,
        device: Device,
        executable: std::path::PathBuf,
        desktop_size: (u32, u32),
        saved: credentials::Lookup,
    ) -> Result<()> {
        // Settings may have changed while the authentication UI was open.
        // Never send a credential to an edited or removed device.
        if !self.ivars().store.borrow().config.devices.contains(&device) {
            return Ok(());
        }
        let password = match saved {
            credentials::Lookup::Cancelled => return Ok(()),
            credentials::Lookup::Saved(password) => password,
            credentials::Lookup::Missing => {
                let Some((password, remember)) = self.ask_password(&device) else {
                    return Ok(());
                };
                anyhow::ensure!(
                    !password.contains(['\r', '\n', '\0']),
                    "Passwords cannot contain line breaks or NUL characters."
                );
                if remember {
                    credentials::set(device.id, &password)?;
                }
                if remember != device.remember_password {
                    let mut devices = self.ivars().store.borrow().config.devices.clone();
                    if let Some(saved) = devices.iter_mut().find(|d| d.id == device.id) {
                        saved.remember_password = remember;
                    }
                    self.ivars().store.borrow_mut().commit(devices)?;
                }
                password
            }
        };
        let child = rdp::start(executable, &device, &password, desktop_size)?;
        // Remember only after the password prompt and helper launch succeed.
        let history_result = self
            .ivars()
            .store
            .borrow_mut()
            .remember_connection(device.id);
        self.ivars().sessions.borrow_mut().insert(device.id);
        self.refresh_connect();
        std::thread::spawn(move || {
            let result = rdp::wait(child, &password).map_err(|e| format!("{e:#}"));
            DispatchQueue::main().exec_async(move || {
                DELEGATE.with(|cell| {
                    if let Some(delegate) = cell.get() {
                        delegate.ivars().sessions.borrow_mut().remove(&device.id);
                        delegate.refresh_connect();
                        if let Err(error) = result {
                            alert(
                                delegate.mtm(),
                                &format!("Connection to {} ended", device.name),
                                &error,
                                &["OK"],
                            );
                        }
                    }
                });
            });
        });
        if let Err(error) = history_result {
            self.show_error("Couldn’t save connection history", &error);
        }
        Ok(())
    }

    fn ask_password(&self, device: &Device) -> Option<(String, bool)> {
        let mtm = self.mtm();
        activate(mtm);
        let dialog = NSAlert::new(mtm);
        dialog.setMessageText(&NSString::from_str(&format!("Connect to {}", device.name)));
        dialog.setInformativeText(&NSString::from_str(&format!(
            "Enter the password for {} at {}.",
            device.username,
            device.address()
        )));
        dialog.addButtonWithTitle(ns_string!("Connect"));
        dialog.addButtonWithTitle(ns_string!("Cancel"));
        let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 330.0, 78.0));
        let password = NSSecureTextField::initWithFrame(
            NSSecureTextField::alloc(mtm),
            rect(0.0, 44.0, 330.0, 26.0),
        );
        password.setPlaceholderString(Some(ns_string!("Password")));
        password.setAccessibilityLabel(Some(ns_string!("Password")));
        let remember = checkbox(mtm, "Remember in Keychain", rect(0.0, 8.0, 330.0, 24.0));
        remember.setState(state(device.remember_password));
        view.addSubview(&password);
        view.addSubview(&remember);
        dialog.setAccessoryView(Some(&view));
        dialog.window().setInitialFirstResponder(Some(&password));
        if dialog.runModal() == NSAlertFirstButtonReturn {
            Some((
                password.stringValue().to_string(),
                remember.state() == NSControlStateValueOn,
            ))
        } else {
            None
        }
    }

    fn show_error(&self, title: &str, error: &anyhow::Error) {
        alert(self.mtm(), title, &format!("{error:#}"), &["OK"]);
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}
fn state(on: bool) -> NSControlStateValue {
    if on {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    }
}

fn label(
    mtm: MainThreadMarker,
    parent: &NSView,
    text: &str,
    frame: NSRect,
    size: f64,
    bold: bool,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setFrame(frame);
    let font = if bold {
        NSFont::boldSystemFontOfSize(size)
    } else {
        NSFont::systemFontOfSize(size)
    };
    label.setFont(Some(&font));
    if size <= 12.0 {
        label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    }
    parent.addSubview(&label);
    label
}

fn field(
    mtm: MainThreadMarker,
    parent: &NSView,
    title: &str,
    placeholder: &str,
    frame: NSRect,
) -> Retained<NSTextField> {
    label(
        mtm,
        parent,
        title,
        rect(
            frame.origin.x,
            frame.origin.y + 29.0,
            frame.size.width,
            20.0,
        ),
        12.0,
        false,
    );
    let field = NSTextField::initWithFrame(NSTextField::alloc(mtm), frame);
    field.setPlaceholderString(Some(&NSString::from_str(placeholder)));
    field.setAccessibilityLabel(Some(&NSString::from_str(title)));
    parent.addSubview(&field);
    field
}

fn checkbox(mtm: MainThreadMarker, title: &str, frame: NSRect) -> Retained<NSButton> {
    // SAFETY: A checkbox with no target/action manages its own state.
    let button = unsafe {
        NSButton::checkboxWithTitle_target_action(&NSString::from_str(title), None, None, mtm)
    };
    button.setFrame(frame);
    button
}

fn activate(mtm: MainThreadMarker) {
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
}

fn alert(mtm: MainThreadMarker, title: &str, message: &str, buttons: &[&str]) -> NSModalResponse {
    activate(mtm);
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(message));
    for button in buttons {
        alert.addButtonWithTitle(&NSString::from_str(button));
    }
    alert.runModal()
}

pub fn run() {
    let mtm = MainThreadMarker::new().expect("tinyRDP must start on the main thread.");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    #[cfg(debug_assertions)]
    let test_directory = std::env::args()
        .any(|arg| arg == "--smoke-test")
        .then(|| std::env::temp_dir().join(format!("minirdp-ui-{}", Uuid::new_v4())));
    #[cfg(debug_assertions)]
    let opened = if let Some(directory) = &test_directory {
        Store::at(directory.join("devices.json"))
    } else {
        Store::open()
    };
    #[cfg(not(debug_assertions))]
    let opened = Store::open();
    let store = match opened {
        Ok(store) => store,
        Err(error) => {
            alert(
                mtm,
                "Couldn’t open tinyRDP",
                &format!("{error:#}"),
                &["Exit"],
            );
            return;
        }
    };
    let delegate = AppDelegate::new(mtm, store);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    DELEGATE.with(|cell| {
        cell.set(delegate).ok();
    });
    app.run();
    #[cfg(debug_assertions)]
    if let Some(directory) = test_directory {
        let _ = std::fs::remove_dir_all(directory);
    }
}
