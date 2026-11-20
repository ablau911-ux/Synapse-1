//! #2361: pure authorization, receiver binding and literal UTF-16 key records.
pub(crate) type ReceiverIdentity = (i64, u32, u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConsoleTypeRefusal {
    Session,
    Ownership,
    Target,
    Claim,
    Class,
    Identity,
    Empty,
    Control,
    TooLarge,
    Membership,
    Coordinates,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConsoleTypeOutcome {
    Queued(usize),
    Refused,
    Uncertain,
}

#[derive(Debug)]
pub(crate) struct ConsoleTypePlan {
    receiver: ReceiverIdentity,
    text: String,
}

impl ConsoleTypePlan {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        session: Option<&str>,
        owner: Option<&str>,
        target: Option<i64>,
        claimed: bool,
        class: &str,
        saved: Option<ReceiverIdentity>,
        observed: Option<ReceiverIdentity>,
        text: &str,
        coordinates: bool,
    ) -> Result<Self, ConsoleTypeRefusal> {
        let session = session
            .filter(|value| !value.trim().is_empty())
            .ok_or(ConsoleTypeRefusal::Session)?;
        if owner != Some(session) {
            return Err(ConsoleTypeRefusal::Ownership);
        }
        let target = target
            .filter(|hwnd| *hwnd > 0)
            .ok_or(ConsoleTypeRefusal::Target)?;
        if !claimed {
            return Err(ConsoleTypeRefusal::Claim);
        }
        if class != "ConsoleWindowClass" {
            return Err(ConsoleTypeRefusal::Class);
        }
        let receiver = saved
            .filter(|(hwnd, pid, created)| *hwnd == target && *pid != 0 && *created != 0)
            .ok_or(ConsoleTypeRefusal::Identity)?;
        if observed != Some(receiver) {
            return Err(ConsoleTypeRefusal::Identity);
        }
        if coordinates {
            return Err(ConsoleTypeRefusal::Coordinates);
        }
        if text.is_empty() {
            return Err(ConsoleTypeRefusal::Empty);
        }
        if text.chars().any(char::is_control) {
            return Err(ConsoleTypeRefusal::Control);
        }
        if text.encode_utf16().count() > crate::m2::MAX_ACT_TYPE_UTF16_UNITS {
            return Err(ConsoleTypeRefusal::TooLarge);
        }
        Ok(Self {
            receiver,
            text: text.to_owned(),
        })
    }

    pub(crate) fn identity(&self) -> ReceiverIdentity {
        self.receiver
    }

    pub(crate) fn key_records(&self) -> Vec<(bool, u16)> {
        self.text
            .encode_utf16()
            .flat_map(|unit| [(true, unit), (false, unit)])
            .collect()
    }

    pub(crate) fn helper_outcome(&self, code: Option<i32>) -> ConsoleTypeOutcome {
        match code {
            Some(0) => ConsoleTypeOutcome::Queued(self.text.encode_utf16().count() * 2),
            Some(2) => ConsoleTypeOutcome::Refused,
            _ => ConsoleTypeOutcome::Uncertain,
        }
    }

    pub(crate) fn confirm_receiver(
        &self,
        observed: Option<ReceiverIdentity>,
        clients: &[u32],
    ) -> Result<(), ConsoleTypeRefusal> {
        if observed != Some(self.receiver) {
            return Err(ConsoleTypeRefusal::Identity);
        }
        if !clients.contains(&self.receiver.1) {
            return Err(ConsoleTypeRefusal::Membership);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "console_type_table_2361.rs"]
mod console_type_table_2361;
