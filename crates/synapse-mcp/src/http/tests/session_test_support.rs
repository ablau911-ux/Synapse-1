//! Test-only builders for `http::session` regression tests (#13).

use super::{SessionAdmission, SessionRequestState};

impl SessionRequestState {
    pub(super) fn with_admission(mut self, admission: SessionAdmission) -> Self {
        self.admission = admission;
        self
    }
}
