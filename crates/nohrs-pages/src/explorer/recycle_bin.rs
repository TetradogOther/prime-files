//! The Recycle Bin, opened here instead of in Windows' Explorer (issue #4):
//! what is in it, with the name each item had and the date it went there,
//! and Restore, Delete permanently and Empty in its menu.
//!
//! The items come from the `trash` crate through `nohrs_services`' store,
//! the same one Ctrl+Z restores with. Each row's path is the item's own file
//! inside `$Recycle.Bin`, so the preview shows it as any other file.

use std::path::Path;

use gpui::Context;
use nohrs_core::{i18n, k, t, tf};
use nohrs_services::fs::listing::FileEntryDto;
use nohrs_services::fs::trash;

use super::ExplorerPane;
use super::file_ops::OpReport;
use super::types::StatusLevel;

/// The Recycle Bin's address, as the Shell parses it.
#[cfg(windows)]
pub const RECYCLE_BIN: &str = "::{645FF040-5081-101B-9F08-00AA002F954E}";

/// The Recycle Bin's address.
///
/// On Linux the trash is a folder of its own (`~/.local/share/Trash`), but it
/// is not a folder the user browses: it is listed here, with the name each
/// item had and the date it went there, as the Windows one is.
#[cfg(not(windows))]
pub const RECYCLE_BIN: &str = "::trash";

/// What the Recycle Bin is called before the Shell's own name is read.
pub const RECYCLE_BIN_NAME: &str = k!("Lixeira");

/// Whether `path` is the Recycle Bin's address.
pub fn is_recycle_bin(path: &str) -> bool {
    path.eq_ignore_ascii_case(RECYCLE_BIN)
}

// The store, on the thread it is used on: the `trash` crate initializes COM
// as a single-threaded apartment there.
fn store() -> Result<Box<dyn trash::Store>, String> {
    trash::default_store(trash::open_ledger).map_err(|error| error.to_string())
}

/// The rows of the Recycle Bin, newest first. Runs on the listing's own
/// thread (never gpui's pool, where the crate's COM setup panics).
#[allow(clippy::disallowed_methods)]
pub(super) fn read_entries() -> Result<Vec<FileEntryDto>, String> {
    let items = store()?.list().map_err(|error| error.to_string())?;
    Ok(items
        .into_iter()
        .map(|item| {
            let size = if item.is_dir {
                0
            } else {
                std::fs::metadata(&item.id).map_or(0, |metadata| metadata.len())
            };
            FileEntryDto {
                name: item.file_name(),
                path: item.id.clone(),
                kind: if item.is_dir { "dir" } else { "file" }.to_string(),
                size,
                modified: u64::try_from(item.deleted_at_unix).unwrap_or(0),
                attributes: 0,
            }
        })
        .collect())
}

enum Action {
    Restore,
    Purge,
}

// Runs `action` on the items whose ids are `ids` (all of them when `None`),
// on a thread of its own.
fn run(action: Action, ids: Option<Vec<String>>) -> OpReport {
    let worker = std::thread::Builder::new()
        .name("recycle-bin".into())
        .spawn(move || {
            let mut report = OpReport::default();
            let mut store = match store() {
                Ok(store) => store,
                Err(error) => {
                    report.failed(error);
                    return report;
                }
            };
            let items = match store.list() {
                Ok(items) => items,
                Err(error) => {
                    report.failed(error.to_string());
                    return report;
                }
            };
            for item in items {
                if let Some(ids) = &ids
                    && !ids.iter().any(|id| id.eq_ignore_ascii_case(&item.id))
                {
                    continue;
                }
                let done = match action {
                    Action::Restore => store.restore(&item),
                    Action::Purge => store.purge(&item),
                };
                match done {
                    Ok(()) => {}
                    Err(error) => {
                        report.failed(format!("{}: {error}", item.original_path.display()))
                    }
                }
            }
            report
        });
    match worker.map(|thread| thread.join()) {
        Ok(Ok(report)) => report,
        Ok(Err(_)) | Err(_) => {
            let mut report = OpReport::default();
            report.failed(t!("A Lixeira não respondeu").to_string());
            report
        }
    }
}

impl ExplorerPane {
    /// Puts the selected items of the Recycle Bin back where they were.
    pub(crate) fn restore_from_recycle_bin(&mut self, cx: &mut Context<Self>) {
        let ids = self.selected_paths();
        if ids.is_empty() {
            return;
        }
        let total = ids.len();
        let label = i18n::plural(
            total,
            k!("{n} item restaurado"),
            k!("{n} itens restaurados"),
        );
        self.run_fs_op(total, label, cx, move || run(Action::Restore, Some(ids)));
    }

    /// Deletes the selected items of the Recycle Bin for good.
    pub(crate) fn purge_from_recycle_bin(&mut self, cx: &mut Context<Self>) {
        let ids = self.selected_paths();
        if ids.is_empty() {
            return;
        }
        let total = ids.len();
        let label = i18n::plural(total, k!("{n} item excluído"), k!("{n} itens excluídos"));
        self.run_fs_op(total, label, cx, move || run(Action::Purge, Some(ids)));
    }

    /// Empties the Recycle Bin.
    pub(crate) fn empty_recycle_bin(&mut self, cx: &mut Context<Self>) {
        let total = self.entries.len();
        if total == 0 {
            self.set_status(StatusLevel::Info, t!("A Lixeira já está vazia").to_string());
            cx.notify();
            return;
        }
        let label = i18n::plural(total, k!("{n} item excluído"), k!("{n} itens excluídos"));
        self.run_fs_op(total, label, cx, move || run(Action::Purge, None));
    }

    /// Answers a double click on an item of the Recycle Bin: it cannot be
    /// opened from there, as in the Explorer.
    pub(crate) fn explain_recycle_bin_item(&mut self, path: &str, cx: &mut Context<Self>) {
        let name = Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.set_status(
            StatusLevel::Info,
            tf!(
                "Para abrir {name}, restaure o item primeiro (botão direito › Restaurar).",
                name = name
            ),
        );
        cx.notify();
    }
}
