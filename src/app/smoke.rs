//! Exercises real AppKit controls on the main thread with an isolated device file.
use super::*;
use objc2_foundation::NSDictionary;

pub(super) fn run(delegate: &AppDelegate) -> Result<()> {
    let status_button = delegate
        .ivars()
        .status_item
        .get()
        .unwrap()
        .button(delegate.mtm())
        .unwrap();
    let status_image = status_button
        .image()
        .context("The menu bar icon is missing.")?;
    assert!(status_image.isTemplate());
    assert_eq!(status_image.size(), NSSize::new(18.0, 18.0));
    capture_preview(&status_button, "menu-bar-preview.png")?;
    let status_menu = delegate
        .ivars()
        .status_item
        .get()
        .unwrap()
        .menu(delegate.mtm())
        .unwrap();
    assert_eq!(status_menu.numberOfItems(), 6);
    let history = status_menu.itemAtIndex(0).unwrap();
    assert_eq!(history.title().to_string(), "No history");
    assert!(!history.isEnabled());
    assert!(status_menu.itemAtIndex(1).unwrap().isSeparatorItem());
    assert_eq!(
        status_menu.itemAtIndex(2).unwrap().title().to_string(),
        "Connect"
    );
    assert_eq!(
        status_menu.itemAtIndex(3).unwrap().title().to_string(),
        "Settings"
    );
    assert!(status_menu.itemAtIndex(4).unwrap().isSeparatorItem());
    assert_eq!(
        status_menu.itemAtIndex(5).unwrap().title().to_string(),
        "Exit"
    );
    let menu = delegate.ivars().connect_menu.get().unwrap();
    assert_eq!(menu.numberOfItems(), 2); // Empty state plus Add a device.
    delegate.show_editor();
    let (width, height) = delegate.desktop_size()?;
    anyhow::ensure!(
        width > 0 && height > 0,
        "Screen detection returned an invalid resolution."
    );
    println!("Detected desktop resolution: {width}x{height}");
    println!("Remote desktop helper: {}", rdp::backend()?.display());
    let editor = delegate.ivars().editor.get().unwrap();
    assert_eq!(
        editor.window.title().to_string(),
        "tinyRDP · Manage devices"
    );
    assert_eq!(editor.table.numberOfRows(), 0);
    assert_eq!(editor.resolution.indexOfSelectedItem(), 0);
    assert!(editor.resolution_width.isHidden());
    assert_eq!(editor.invert_scroll.state(), NSControlStateValueOff);

    editor.name.setStringValue(ns_string!("Work PC"));
    editor.host.setStringValue(ns_string!("192.168.1.10"));
    editor.username.setStringValue(ns_string!("alex"));
    editor.remember.setState(NSControlStateValueOff);
    editor.invert_scroll.setState(NSControlStateValueOn);
    let folder_test_directory =
        std::env::temp_dir().join(format!("minirdp-folder-ui-{}", Uuid::new_v4()));
    let first_folder = folder_test_directory.join("first/Projects");
    let second_folder = folder_test_directory.join("second/Projects");
    std::fs::create_dir_all(&first_folder)?;
    std::fs::create_dir_all(&second_folder)?;
    let picker = delegate.folder_picker();
    assert!(picker.canChooseDirectories());
    assert!(!picker.canChooseFiles());
    assert!(picker.allowsMultipleSelection());
    assert!(!picker.canCreateDirectories());
    delegate.begin_shared_folders();
    let folder_editor = delegate.ivars().folder_editor.get().unwrap();
    assert_eq!(folder_editor.table.numberOfRows(), 0);
    delegate.add_shared_folder_paths(vec![first_folder.clone(), second_folder.clone()])?;
    assert_eq!(folder_editor.table.numberOfRows(), 2);
    assert_eq!(folder_editor.draft.borrow()[0].name, "Projects");
    assert_eq!(folder_editor.draft.borrow()[1].name, "Projects (2)");
    assert!(
        delegate
            .add_shared_folder_paths(vec![first_folder.clone()])
            .is_err()
    );
    assert_eq!(folder_editor.draft.borrow().len(), 2);
    delegate.rename_shared_folder(0, "Work docs")?;
    assert!(delegate.rename_shared_folder(1, "WORK DOCS").is_err());
    folder_editor.table.reloadData();
    let folder_preview = folder_editor.alert.accessoryView().unwrap();
    capture_preview(&folder_preview, "shared-folders-preview.png")?;
    delegate.finish_shared_folders(false)?;
    assert!(editor.shared_folders.borrow().is_empty());
    delegate.begin_shared_folders();
    assert!(folder_editor.draft.borrow().is_empty());
    delegate.add_shared_folder_paths(vec![first_folder.clone()])?;
    let column = folder_editor.table.tableColumns().objectAtIndex(0);
    // SAFETY: Invoke the same data-source action as editing a share name inline.
    let _: () = unsafe {
        msg_send![delegate,
            tableView: &*folder_editor.table,
            setObjectValue: &*ns_string!("Project files"),
            forTableColumn: &*column,
            row: 0isize
        ]
    };
    assert_eq!(folder_editor.draft.borrow()[0].name, "Project files");
    delegate.finish_shared_folders(true)?;
    assert_eq!(editor.shared_folders.borrow().len(), 1);
    assert!(editor.folders_button.title().to_string().contains("(1)"));
    assert_ne!(delegate.draft()?, *delegate.ivars().baseline.borrow());
    editor
        .resolution
        .selectItemWithTitle(ns_string!("1920 × 1080"));
    // SAFETY: Invoke the same action AppKit sends when selecting a resolution.
    let _: () = unsafe { msg_send![delegate, resolutionChanged: None::<&AnyObject>] };
    delegate.save_device()?;
    let first = delegate.ivars().baseline.borrow().id;
    assert_eq!(delegate.ivars().baseline.borrow().shared_folders.len(), 1);
    // Saving a device alone must not create connection history.
    assert_eq!(history.title().to_string(), "No history");
    delegate
        .ivars()
        .store
        .borrow_mut()
        .remember_connection(first)?;
    delegate.refresh_connect();
    assert_eq!(history.title().to_string(), "Work PC");
    assert!(history.isEnabled());
    assert_eq!(history.action(), Some(sel!(connectDevice:)));
    assert_eq!(
        history
            .representedObject()
            .unwrap()
            .downcast::<NSString>()
            .unwrap()
            .to_string(),
        first.to_string()
    );
    assert!(delegate.ivars().baseline.borrow().invert_scroll);
    assert_eq!(editor.invert_scroll.state(), NSControlStateValueOn);
    assert_eq!(editor.table.numberOfRows(), 1);
    assert_eq!(menu.numberOfItems(), 1);
    assert_eq!(editor.table.selectedRow(), 0);
    assert_eq!(
        delegate.ivars().baseline.borrow().resolution,
        Some(Resolution {
            width: 1920,
            height: 1080
        })
    );

    // SAFETY: Invoke the same action that AppKit sends for the Add button.
    let _: () = unsafe { msg_send![delegate, addDevice: None::<&AnyObject>] };
    assert_eq!(editor.resolution.indexOfSelectedItem(), 0);
    assert_eq!(editor.invert_scroll.state(), NSControlStateValueOff);
    assert!(editor.shared_folders.borrow().is_empty());
    editor.name.setStringValue(ns_string!("Home server"));
    editor.host.setStringValue(ns_string!("[2001:db8::1]"));
    editor.username.setStringValue(ns_string!("administrator"));
    editor.port.setStringValue(ns_string!("3390"));
    editor.remember.setState(NSControlStateValueOff);
    editor.resolution.selectItemWithTitle(ns_string!("Custom…"));
    // SAFETY: This is the resolution popup's registered action.
    let _: () = unsafe { msg_send![delegate, resolutionChanged: None::<&AnyObject>] };
    assert!(!editor.resolution_width.isHidden());
    assert!(!editor.resolution_height.isHidden());
    editor.resolution_width.setStringValue(ns_string!("0"));
    editor.resolution_height.setStringValue(ns_string!("1117"));
    assert!(delegate.save_device().is_err());
    assert_eq!(delegate.ivars().store.borrow().config.devices.len(), 1);
    editor.resolution_width.setStringValue(ns_string!("1728"));
    delegate.save_device()?;
    let second = delegate.ivars().baseline.borrow().id;
    assert_eq!(history.title().to_string(), "Work PC");
    assert_ne!(first, second);
    assert_eq!(editor.table.numberOfRows(), 2);
    assert_eq!(menu.numberOfItems(), 2);
    assert_eq!(
        delegate.ivars().baseline.borrow().resolution,
        Some(Resolution {
            width: 1728,
            height: 1117
        })
    );

    delegate.select_device(Some(first));
    assert_eq!(editor.shared_folders.borrow()[0].name, "Project files");
    assert_eq!(
        editor.shared_folders.borrow()[0].path,
        first_folder.canonicalize()?
    );
    assert_eq!(editor.invert_scroll.state(), NSControlStateValueOn);
    assert_eq!(editor.host.stringValue().to_string(), "192.168.1.10");
    assert_eq!(
        editor.resolution.titleOfSelectedItem().unwrap().to_string(),
        "1920 × 1080"
    );
    assert!(editor.resolution_width.isHidden());
    editor.host.setStringValue(ns_string!("192.168.1.20"));
    editor.name.setStringValue(ns_string!("Office PC"));
    delegate.save_device()?;
    assert_eq!(history.title().to_string(), "Office PC");
    assert_eq!(delegate.ivars().baseline.borrow().id, first);
    assert_eq!(delegate.ivars().store.borrow().config.devices.len(), 2);
    assert_eq!(editor.heading.stringValue().to_string(), "Edit device");

    delegate.begin_shared_folders();
    folder_editor
        .table
        .selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(0), false);
    delegate.remove_selected_shared_folder();
    assert!(folder_editor.draft.borrow().is_empty());
    assert!(first_folder.is_dir());
    delegate.finish_shared_folders(false)?;
    assert_eq!(editor.shared_folders.borrow().len(), 1);
    delegate.begin_shared_folders();
    folder_editor
        .table
        .selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(0), false);
    delegate.remove_selected_shared_folder();
    delegate.finish_shared_folders(true)?;
    delegate.save_device()?;
    assert!(delegate.ivars().baseline.borrow().shared_folders.is_empty());
    assert!(first_folder.is_dir());

    delegate.select_device(Some(second));
    assert_eq!(editor.invert_scroll.state(), NSControlStateValueOff);
    editor.invert_scroll.setState(NSControlStateValueOn);
    assert_ne!(delegate.draft()?, *delegate.ivars().baseline.borrow());
    delegate.save_device()?;
    assert!(delegate.ivars().baseline.borrow().invert_scroll);
    delegate.select_device(Some(first));
    delegate.select_device(Some(second));
    assert_eq!(editor.invert_scroll.state(), NSControlStateValueOn);
    editor.invert_scroll.setState(NSControlStateValueOff);
    delegate.save_device()?;
    assert!(!delegate.ivars().baseline.borrow().invert_scroll);
    assert_eq!(
        editor.resolution.titleOfSelectedItem().unwrap().to_string(),
        "Custom…"
    );
    assert_eq!(editor.resolution_width.stringValue().to_string(), "1728");
    assert_eq!(editor.resolution_height.stringValue().to_string(), "1117");
    assert!(!editor.resolution_width.isHidden());

    delegate
        .ivars()
        .store
        .borrow_mut()
        .remember_connection(second)?;
    delegate.refresh_connect();
    assert_eq!(history.title().to_string(), "Home server");
    assert_eq!(
        history
            .representedObject()
            .unwrap()
            .downcast::<NSString>()
            .unwrap()
            .to_string(),
        second.to_string()
    );
    let pending_device = delegate.ivars().baseline.borrow().clone();
    let history_before_authentication =
        delegate.ivars().store.borrow().config.last_connected_device;
    // Authentication cancellation must not launch even a helper, show a remote
    // password prompt, change history, or leave the device disabled. A missing
    // executable would make this check fail if the launch path were reached.
    delegate.ivars().connecting.borrow_mut().insert(second);
    delegate.refresh_connect();
    assert_eq!(history.title().to_string(), "Home server (connecting…)");
    assert!(!history.isEnabled());
    delegate.finish_connect(
        pending_device.clone(),
        "/nonexistent/tinyrdp-auth-test".into(),
        (1920, 1080),
        Ok(credentials::Lookup::Cancelled),
    )?;
    assert!(!delegate.ivars().connecting.borrow().contains(&second));
    assert!(delegate.ivars().sessions.borrow().is_empty());
    assert!(history.isEnabled());
    delegate.ivars().connecting.borrow_mut().insert(second);
    assert!(
        delegate
            .finish_connect(
                pending_device.clone(),
                "/nonexistent/tinyrdp-auth-test".into(),
                (1920, 1080),
                Err(anyhow::anyhow!("Simulated authentication failure")),
            )
            .is_err()
    );
    assert!(!delegate.ivars().connecting.borrow().contains(&second));
    assert!(delegate.ivars().sessions.borrow().is_empty());
    // An old connection request cannot send a credential to a changed device.
    let mut changed_device = pending_device;
    changed_device.host = "changed.example.test".into();
    delegate.ivars().connecting.borrow_mut().insert(second);
    delegate.finish_connect(
        changed_device,
        "/nonexistent/tinyrdp-auth-test".into(),
        (1920, 1080),
        Ok(credentials::Lookup::Saved("test password".into())),
    )?;
    assert!(delegate.ivars().sessions.borrow().is_empty());
    assert_eq!(
        delegate.ivars().store.borrow().config.last_connected_device,
        history_before_authentication
    );
    assert!(history.isEnabled());
    delegate.ivars().sessions.borrow_mut().insert(second);
    delegate.refresh_connect();
    assert_eq!(history.title().to_string(), "Home server (session open)");
    assert!(!history.isEnabled());
    delegate.ivars().sessions.borrow_mut().remove(&second);
    delegate.refresh_connect();
    assert_eq!(history.title().to_string(), "Home server");
    assert!(history.isEnabled());

    let view = editor.window.contentView().unwrap();
    editor.window.displayIfNeeded();
    let path = capture_preview(&view, "ui-preview.png")?;

    editor
        .resolution
        .selectItemWithTitle(ns_string!("Match current screen"));
    // SAFETY: This is the resolution popup's registered action.
    let _: () = unsafe { msg_send![delegate, resolutionChanged: None::<&AnyObject>] };
    delegate.save_device()?;
    assert_eq!(delegate.ivars().baseline.borrow().resolution, None);
    assert!(editor.resolution_width.isHidden());

    delegate.delete_saved_device(second)?;
    assert_eq!(history.title().to_string(), "No history");
    assert!(!history.isEnabled());
    assert!(history.representedObject().is_none());
    delegate.delete_saved_device(first)?;
    assert!(delegate.ivars().store.borrow().config.devices.is_empty());
    assert_eq!(editor.table.numberOfRows(), 0);
    assert_eq!(menu.numberOfItems(), 2);
    assert!(!editor.remove.isEnabled());
    println!(
        "Native UI smoke test passed: device management, resolution, scrolling, history, shared-folder add/edit/remove/cancel, folder-picker configuration, and per-device isolation."
    );
    println!("Preview: {}", path.display());
    std::fs::remove_dir_all(folder_test_directory)?;
    NSApplication::sharedApplication(delegate.mtm()).stop(None);
    Ok(())
}

fn capture_preview(view: &NSView, name: &str) -> Result<std::path::PathBuf> {
    view.layoutSubtreeIfNeeded();
    let image = view
        .bitmapImageRepForCachingDisplayInRect(view.bounds())
        .context("Could not capture the native view.")?;
    view.cacheDisplayInRect_toBitmapImageRep(view.bounds(), &image);
    // SAFETY: An empty properties dictionary is valid for a PNG representation.
    let png = unsafe {
        image.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .context("Could not encode the UI preview.")?;
    let path = std::env::current_dir()?.join("target").join(name);
    anyhow::ensure!(
        png.writeToFile_atomically(&NSString::from_str(&path.to_string_lossy()), true),
        "Could not save the preview."
    );
    Ok(path)
}
