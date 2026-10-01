/// Marker for types that are `Sync` but not `Send`
#[expect(dead_code)]
pub(crate) struct SyncNotSend(*mut ());

unsafe impl Sync for SyncNotSend {}

pub(crate) struct NotSendOrSync(#[expect(dead_code)] *mut ());
