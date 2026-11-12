//! Trascinamento in uscita: una firma salvata verso Word, Outlook o Esplora.
//! Il dato e' quello che darebbe Esplora per lo stesso file (IDataObject della shell),
//! cosi' ogni destinazione lo tratta come un PNG trascinato da una cartella.

use std::path::Path;

use windows::core::{implement, BOOL, HRESULT, HSTRING};
use windows::Win32::Foundation::{DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, S_OK};
use windows::Win32::System::Com::{IBindCtx, IDataObject};
use windows::Win32::System::Ole::{DoDragDrop, IDropSource, IDropSource_Impl, DROPEFFECT, DROPEFFECT_COPY};
use windows::Win32::System::SystemServices::{MODIFIERKEYS_FLAGS, MK_LBUTTON};
use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName, BHID_DataObject};

#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if (keys & MK_LBUTTON).0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Trascina il file finche' l'utente non rilascia il tasto. Vero se e' stato rilasciato su una destinazione.
/// Va chiamata nel thread della barra, dove OLE e' gia' inizializzato, mentre il tasto sinistro e' premuto.
pub(crate) fn drag_file(path: &Path) -> bool {
    unsafe {
        let path = HSTRING::from(path.as_os_str());
        let Ok(item) = SHCreateItemFromParsingName::<_, Option<&IBindCtx>, IShellItem>(&path, None) else {
            return false;
        };
        let Ok(data) = item.BindToHandler::<Option<&IBindCtx>, IDataObject>(None, &BHID_DataObject) else {
            return false;
        };
        let source: IDropSource = DropSource.into();
        let mut effect = DROPEFFECT(0);
        DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect) == DRAGDROP_S_DROP && effect.0 != 0
    }
}
