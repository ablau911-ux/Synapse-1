/// Close normally unless admission or daemon restart deliberately abandons the socket.
pub(super) struct SocketClosePolicy {
    abortive: bool,
}

impl SocketClosePolicy {
    pub(super) const fn for_drop(shutdown_on_drop: bool) -> Self {
        Self {
            abortive: shutdown_on_drop,
        }
    }

    pub(super) const fn capacity_rejected() -> Self {
        Self { abortive: true }
    }

    pub(super) const fn is_abortive(&self) -> bool {
        self.abortive
    }
}

#[cfg(test)]
#[path = "socket_close_policy_table_2427.rs"]
mod socket_close_policy_table_2427;
