//! Native shared-folder editor. Changes remain in the device draft until Save.
use super::*;
use std::path::PathBuf;

pub(super) struct FolderEditor {
    pub alert: Retained<NSAlert>,
    pub table: Retained<NSTableView>,
    pub remove: Retained<NSButton>,
    pub feedback: Retained<NSTextField>,
    pub draft: RefCell<Vec<SharedFolder>>,
}

impl AppDelegate {
    pub(super) fn is_folder_table(&self, table: &NSTableView) -> bool {
        self.ivars()
            .folder_editor
            .get()
            .is_some_and(|editor| std::ptr::eq(table, &*editor.table))
    }

    fn build_folder_editor(&self) -> FolderEditor {
        let mtm = self.mtm();
        let alert = NSAlert::new(mtm);
        alert.setMessageText(ns_string!("Shared folders"));
        alert.setInformativeText(ns_string!("Both computers work on the same Mac files, including edits and deletions. Double-click a share name to edit it. Save the device and reconnect to apply changes."));
        alert.addButtonWithTitle(ns_string!("Done"));
        alert.addButtonWithTitle(ns_string!("Cancel"));
        let content = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 640.0, 270.0));
        let scroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(0.0, 54.0, 640.0, 216.0));
        scroll.setHasVerticalScroller(true);
        scroll.setBorderType(NSBorderType::BezelBorder);
        let table =
            NSTableView::initWithFrame(NSTableView::alloc(mtm), rect(0.0, 0.0, 636.0, 216.0));
        for (identifier, title, width, editable) in [
            ("shareName", "Share name", 180.0, true),
            ("folderPath", "Mac folder", 426.0, false),
        ] {
            let column = NSTableColumn::initWithIdentifier(
                NSTableColumn::alloc(mtm),
                &NSString::from_str(identifier),
            );
            column.setTitle(&NSString::from_str(title));
            column.setWidth(width);
            column.setEditable(editable);
            table.addTableColumn(&column);
        }
        table.setRowHeight(27.0);
        table.setAllowsMultipleSelection(false);
        table.setAllowsEmptySelection(true);
        table.setAccessibilityLabel(Some(ns_string!("Shared folders")));
        // SAFETY: The delegate implements both protocols and outlives this view.
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(self)));
            table.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        scroll.setDocumentView(Some(&table));
        content.addSubview(&scroll);
        let add = self.button(
            "Add folder…",
            sel!(addSharedFolder:),
            rect(0.0, 10.0, 120.0, 30.0),
        );
        content.addSubview(&add);
        let remove = self.button(
            "Remove",
            sel!(removeSharedFolder:),
            rect(128.0, 10.0, 90.0, 30.0),
        );
        remove.setEnabled(false);
        content.addSubview(&remove);
        let feedback = label(
            mtm,
            &content,
            "",
            rect(232.0, 4.0, 408.0, 42.0),
            11.0,
            false,
        );
        feedback.setMaximumNumberOfLines(2);
        alert.setAccessoryView(Some(&content));
        FolderEditor {
            alert,
            table,
            remove,
            feedback,
            draft: RefCell::new(Vec::new()),
        }
    }

    pub(super) fn begin_shared_folders(&self) {
        let editor = self.ivars().editor.get().unwrap();
        let folders = self
            .ivars()
            .folder_editor
            .get_or_init(|| self.build_folder_editor());
        folders
            .draft
            .replace(editor.shared_folders.borrow().clone());
        folders.feedback.setStringValue(ns_string!(""));
        folders.table.reloadData();
        // SAFETY: No sender is required for the table's standard deselect action.
        unsafe {
            folders.table.deselectAll(None);
        }
        self.update_folder_selection();
    }

    pub(super) fn finish_shared_folders(&self, accepted: bool) -> Result<()> {
        let folders = self.ivars().folder_editor.get().unwrap();
        folders.alert.window().makeFirstResponder(None);
        if accepted {
            let draft = folders.draft.borrow();
            validate_shared_folders(&draft)?;
            self.ivars()
                .editor
                .get()
                .unwrap()
                .shared_folders
                .replace(draft.clone());
            self.update_shared_folders_button();
        }
        Ok(())
    }

    pub(super) fn show_shared_folders(&self) -> Result<()> {
        self.begin_shared_folders();
        let folders = self.ivars().folder_editor.get().unwrap();
        let response = folders.alert.runModal();
        self.finish_shared_folders(response == NSAlertFirstButtonReturn)
    }

    pub(super) fn folder_picker(&self) -> Retained<NSOpenPanel> {
        let panel = NSOpenPanel::openPanel(self.mtm());
        panel.setCanChooseFiles(false);
        panel.setCanChooseDirectories(true);
        panel.setAllowsMultipleSelection(true);
        panel.setCanCreateDirectories(false);
        panel.setResolvesAliases(true);
        panel.setTreatsFilePackagesAsDirectories(false);
        panel.setPrompt(Some(ns_string!("Share folder")));
        panel.setMessage(Some(ns_string!(
            "Choose Mac folders to share with this device."
        )));
        panel
    }

    pub(super) fn choose_shared_folders(&self) -> Result<()> {
        let panel = self.folder_picker();
        if panel.runModal() == NSModalResponseOK {
            let paths = panel
                .URLs()
                .iter()
                .map(|url| {
                    url.path()
                        .map(|path| PathBuf::from(path.to_string()))
                        .context("Could not read the selected folder path.")
                })
                .collect::<Result<Vec<_>>>()?;
            self.add_shared_folder_paths(paths)?;
        }
        Ok(())
    }

    pub(super) fn add_shared_folder_paths(&self, paths: Vec<PathBuf>) -> Result<()> {
        let folders = self.ivars().folder_editor.get().unwrap();
        let mut draft = folders.draft.borrow().clone();
        for path in paths {
            let path = path
                .canonicalize()
                .with_context(|| format!("Cannot open folder: {}", path.display()))?;
            let base = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Shared folder");
            let base: String = base
                .chars()
                .map(|c| {
                    if c.is_control() || "<>:\"/\\|?*".contains(c) {
                        '_'
                    } else {
                        c
                    }
                })
                .collect();
            let base = base.trim().trim_end_matches('.');
            let base = if base.is_empty() {
                "Shared folder"
            } else {
                base
            };
            let mut index = 1;
            let name = loop {
                let suffix = if index == 1 {
                    String::new()
                } else {
                    format!(" ({index})")
                };
                let mut name: String = base.chars().take(64 - suffix.chars().count()).collect();
                name = name.trim_end_matches([' ', '.']).into();
                name.push_str(&suffix);
                if !draft
                    .iter()
                    .any(|folder| folder.name.to_lowercase() == name.to_lowercase())
                {
                    break name;
                }
                index += 1;
            };
            let folder = SharedFolder { name, path };
            folder.validate_access()?;
            draft.push(folder);
        }
        validate_shared_folders(&draft)?;
        folders.draft.replace(draft);
        folders.table.reloadData();
        let count = folders.draft.borrow().len();
        if count > 0 {
            folders.table.selectRowIndexes_byExtendingSelection(
                &NSIndexSet::indexSetWithIndex(count - 1),
                false,
            );
            folders.table.scrollRowToVisible((count - 1) as isize);
        }
        self.folder_feedback("");
        self.update_folder_selection();
        Ok(())
    }

    pub(super) fn rename_shared_folder(&self, row: isize, name: &str) -> Result<()> {
        let folders = self.ivars().folder_editor.get().unwrap();
        let mut draft = folders.draft.borrow().clone();
        let folder = draft
            .get_mut(row as usize)
            .context("Select a shared folder first.")?;
        folder.name = name.trim().into();
        validate_shared_folders(&draft)?;
        folders.draft.replace(draft);
        self.folder_feedback("");
        Ok(())
    }

    pub(super) fn remove_selected_shared_folder(&self) {
        let folders = self.ivars().folder_editor.get().unwrap();
        let row = folders.table.selectedRow();
        if row >= 0 && (row as usize) < folders.draft.borrow().len() {
            folders.draft.borrow_mut().remove(row as usize);
            folders.table.reloadData();
            self.folder_feedback("");
            self.update_folder_selection();
        }
    }

    pub(super) fn update_folder_selection(&self) {
        let folders = self.ivars().folder_editor.get().unwrap();
        folders.remove.setEnabled(folders.table.selectedRow() >= 0);
    }

    pub(super) fn folder_feedback(&self, text: &str) {
        let folders = self.ivars().folder_editor.get().unwrap();
        folders.feedback.setStringValue(&NSString::from_str(text));
        folders.feedback.setToolTip(Some(&NSString::from_str(text)));
    }

    pub(super) fn update_shared_folders_button(&self) {
        let editor = self.ivars().editor.get().unwrap();
        let count = editor.shared_folders.borrow().len();
        let title = if count == 0 {
            "Shared folders…".into()
        } else {
            format!("Shared folders… ({count})")
        };
        editor.folders_button.setTitle(&NSString::from_str(&title));
    }
}
