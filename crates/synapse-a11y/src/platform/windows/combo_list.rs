//! #2348 R3: a classic Win32 combo box (WinForms, MFC or VB6 DropDownList) is
//! picked through its own list messages, sent the way the edit-box text route
//! sends its messages (`SendMessageTimeoutW`, abort if hung, 500 ms): the
//! names are read with CB_GETCOUNT / CB_GETLBTEXT, the pick sets CB_SETCURSEL
//! and tells the parent window CBN_SELCHANGE (what the combo box itself sends
//! when a person picks), and CB_GETCURSEL is read back. No drop-down is opened
//! and no window is brought to the front.

use windows::Win32::UI::WindowsAndMessaging::{
    CB_GETCOUNT, CB_GETCURSEL, CB_GETLBTEXT, CB_GETLBTEXTLEN, CB_SETCURSEL, CBN_SELCHANGE,
    GetDlgCtrlID, GetParent, WM_COMMAND,
};

use super::*;
use crate::{NativeOptionDelivery, NativeOptionReadback};

/// One classic combo box, reached through its own window.
pub(super) struct ComboList {
    id: ElementId,
    hwnd: HWND,
}

impl ComboList {
    /// # Errors
    ///
    /// Refuses a handle that is 0 or no longer a live window.
    pub(super) fn new(id: &ElementId, hwnd: i64) -> A11yResult<Self> {
        let raw = isize::try_from(hwnd).map_err(|err| A11yError::InvalidElementId {
            detail: err.to_string(),
        })?;
        let window = HWND(raw as *mut c_void);
        // SAFETY: IsWindow only reads whether the handle names a live window.
        if raw == 0 || !unsafe { IsWindow(Some(window)) }.as_bool() {
            return Err(A11yError::ElementStale {
                detail: format!(
                    "combo box element {id} has no live window of its own (hwnd 0x{hwnd:x})"
                ),
            });
        }
        Ok(Self {
            id: id.clone(),
            hwnd: window,
        })
    }

    /// Picks the option `pick` means and reads it back.
    ///
    /// # Errors
    ///
    /// `OptionRefused` before anything is sent (no options, no such name, two
    /// with it); a message that fails or times out; a readback that does not
    /// show the chosen option (delivered).
    pub(super) fn pick(&self, pick: &OptionPick) -> A11yResult<NativeOptionReadback> {
        let options = self.options()?;
        let picked = pick
            .choose(&options)
            .map_err(|detail| A11yError::OptionRefused {
                detail: format!("element {}: {detail}", self.id),
            })?;
        let (index, was_selected) = match picked {
            PickedOption::AlreadySelected(index) => (index, true),
            PickedOption::Select(index) => {
                click_dispatch_with(|| self.select(index), || self.read_back(index))?;
                (index, false)
            }
        };
        let name = options
            .get(index)
            .map(|(name, _)| name.clone())
            .unwrap_or_default();
        Ok(NativeOptionReadback::new(
            name,
            index,
            options.len(),
            was_selected,
            true,
            NativeOptionDelivery::ComboBoxMessages,
        ))
    }

    /// Every option's text, and whether it is the selected one, in list order.
    fn options(&self) -> A11yResult<Vec<(String, bool)>> {
        let count = self.send(self.hwnd, CB_GETCOUNT, WPARAM(0), LPARAM(0), "CB_GETCOUNT")?;
        let count = usize::try_from(count).map_err(|_| self.bad_answer("CB_GETCOUNT", count))?;
        let selected = self.send(
            self.hwnd,
            CB_GETCURSEL,
            WPARAM(0),
            LPARAM(0),
            "CB_GETCURSEL",
        )?;
        let mut options = Vec::new();
        for index in 0..count {
            let len = self.send(
                self.hwnd,
                CB_GETLBTEXTLEN,
                WPARAM(index),
                LPARAM(0),
                "CB_GETLBTEXTLEN",
            )?;
            let len = usize::try_from(len).map_err(|_| self.bad_answer("CB_GETLBTEXTLEN", len))?;
            let mut text = vec![0_u16; len.saturating_add(1)];
            let copied = self.send(
                self.hwnd,
                CB_GETLBTEXT,
                WPARAM(index),
                LPARAM(text.as_mut_ptr().cast::<c_void>() as isize),
                "CB_GETLBTEXT",
            )?;
            let copied =
                usize::try_from(copied).map_err(|_| self.bad_answer("CB_GETLBTEXT", copied))?;
            text.truncate(copied.min(len));
            let is_selected = isize::try_from(index).is_ok_and(|at| at == selected);
            options.push((String::from_utf16_lossy(&text), is_selected));
        }
        Ok(options)
    }

    /// CB_SETCURSEL, then the CBN_SELCHANGE a combo box sends its parent when
    /// a person picks (WM_COMMAND: low word = control id, high word =
    /// CBN_SELCHANGE, lParam = the combo box).
    fn select(&self, index: usize) -> A11yResult<()> {
        let answer = self.send(
            self.hwnd,
            CB_SETCURSEL,
            WPARAM(index),
            LPARAM(0),
            "CB_SETCURSEL",
        )?;
        if isize::try_from(index).ok() != Some(answer) {
            return Err(self.bad_answer("CB_SETCURSEL", answer));
        }
        // SAFETY: GetParent and GetDlgCtrlID only read a live window's links.
        let parent = unsafe { GetParent(self.hwnd) }.map_err(|err| {
            A11yError::internal(format!(
                "combo box element {} has no parent window to tell CBN_SELCHANGE: {err}",
                self.id
            ))
        })?;
        // SAFETY: as above.
        let control_id = unsafe { GetDlgCtrlID(self.hwnd) };
        let notice = (CBN_SELCHANGE << 16) | (control_id.cast_unsigned() & 0xFFFF);
        self.send(
            parent,
            WM_COMMAND,
            WPARAM(notice as usize),
            LPARAM(self.hwnd.0 as isize),
            "WM_COMMAND(CBN_SELCHANGE)",
        )?;
        Ok(())
    }

    /// CB_GETCURSEL after the pick must hold the chosen option.
    fn read_back(&self, index: usize) -> A11yResult<()> {
        let now = self.send(
            self.hwnd,
            CB_GETCURSEL,
            WPARAM(0),
            LPARAM(0),
            "CB_GETCURSEL",
        )?;
        if isize::try_from(index).ok() == Some(now) {
            Ok(())
        } else {
            Err(A11yError::internal(format!(
                "CB_SETCURSEL({index}) returned for combo box element {}, but CB_GETCURSEL reads {now}",
                self.id
            )))
        }
    }

    /// One message, sent the way the edit-box text route sends its own.
    fn send(
        &self,
        to: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        operation: &'static str,
    ) -> A11yResult<isize> {
        let mut result = 0_usize;
        send_native_text_message(
            &self.id,
            to,
            message,
            wparam,
            lparam,
            NATIVE_TEXT_MESSAGE_TIMEOUT_MS,
            operation,
            &mut result,
        )?;
        Ok(result as isize)
    }

    fn bad_answer(&self, operation: &str, answer: isize) -> A11yError {
        A11yError::internal(format!(
            "{operation} answered {answer} for combo box element {}",
            self.id
        ))
    }
}
