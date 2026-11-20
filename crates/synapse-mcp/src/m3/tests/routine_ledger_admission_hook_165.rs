//! Test-only, one-shot interleaving at the real ledger writer (#165 round 2).
//! Keep this seam when reverting round-2 production changes for RED evidence.
use std::cell::RefCell;

use synapse_storage::Db;

type Hook = Box<dyn FnOnce(&Db)>;

thread_local! {
    static NEXT_ADMISSION: RefCell<Option<Hook>> = const { RefCell::new(None) };
}

struct Reset;

impl Drop for Reset {
    fn drop(&mut self) {
        let _ = NEXT_ADMISSION.with(|slot| slot.borrow_mut().take());
    }
}

pub(crate) fn with_hook<T>(hook: impl FnOnce(&Db) + 'static, action: impl FnOnce() -> T) -> T {
    NEXT_ADMISSION.with(|slot| {
        let mut slot = slot.borrow_mut();
        assert!(slot.is_none(), "nested ledger admission hook");
        *slot = Some(Box::new(hook));
    });
    let _reset = Reset;
    action()
}

pub(crate) fn run(db: &Db) {
    let hook = NEXT_ADMISSION.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook(db);
    }
}
