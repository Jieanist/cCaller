//! Per-thread param_page memory (FR-A-06, decision Q-06).
//!
//! Every worker thread owns exactly one [`ParamPage`]: a fixed block
//! of [`PARAM_PAGE_SLOTS`] `u64` slots that a wrapper may read and
//! write between calls. The visibility contract is structural - a
//! value written during one Cmd is visible to later Cmds because they
//! run on the same thread with the same page, and it is never visible
//! across threads because pages are never shared (Q-06).
//!
//! Slot indices used by a configuration are validated against the same
//! constant at load time (FR-C-10, `ccaller-core`), so the runtime
//! bounds checks here are defense in depth, not the primary gate.

/// Number of `u64` slots in one thread's param_page
/// (`CCALLER_PARAM_PAGE_SLOTS`, requirement spec 7.1).
pub const PARAM_PAGE_SLOTS: usize = 512;

/// A slot index fell outside `[0, PARAM_PAGE_SLOTS)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("slot index {index} is outside the param_page range [0, {limit})")]
pub struct PageError {
    /// The offending index.
    pub index: i64,
    /// Exclusive upper bound of the valid range (512).
    pub limit: i64,
}

/// One thread's param_page: 512 `u64` slots, zero-initialized.
///
/// The page is passed to every `Call_<name>` invocation; the wrapper
/// alone decides which slots it uses, guided by the `slot_roles` of
/// the library description.
#[derive(Debug, Clone)]
pub struct ParamPage {
    slots: [u64; PARAM_PAGE_SLOTS],
}

impl Default for ParamPage {
    fn default() -> Self {
        Self {
            slots: [0; PARAM_PAGE_SLOTS],
        }
    }
}

impl ParamPage {
    /// A fresh page with every slot zeroed.
    pub fn zeroed() -> Self {
        Self::default()
    }

    /// Write one slot; bounds-checked against
    /// `[0, PARAM_PAGE_SLOTS)`.
    ///
    /// # Errors
    /// Returns [`PageError`] when `index` is negative or at least
    /// [`PARAM_PAGE_SLOTS`] - a condition the load-time analysis
    /// already rejects, so seeing it at runtime means a defect.
    pub fn write(&mut self, index: i64, value: u64) -> Result<(), PageError> {
        let checked = self.checked_index(index)?;
        self.slots[checked] = value;
        Ok(())
    }

    /// Read one slot; bounds-checked against
    /// `[0, PARAM_PAGE_SLOTS)`.
    ///
    /// # Errors
    /// Returns [`PageError`] for the same out-of-range condition as
    /// [`ParamPage::write`].
    pub fn read(&self, index: i64) -> Result<u64, PageError> {
        let checked = self.checked_index(index)?;
        Ok(self.slots[checked])
    }

    /// Raw pointer handed to the unified call signature.
    pub fn as_mut_ptr(&mut self) -> *mut u64 {
        self.slots.as_mut_ptr()
    }

    /// Validate an index and convert it to `usize`.
    fn checked_index(&self, index: i64) -> Result<usize, PageError> {
        if index < 0 || index >= PARAM_PAGE_SLOTS as i64 {
            return Err(PageError {
                index,
                limit: PARAM_PAGE_SLOTS as i64,
            });
        }
        Ok(index as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_page_is_zeroed__F_A_06() {
        let page = ParamPage::zeroed();
        for index in 0..PARAM_PAGE_SLOTS as i64 {
            assert_eq!(page.read(index).unwrap(), 0);
        }
    }

    #[test]
    fn write_then_read_roundtrips__F_A_06() {
        let mut page = ParamPage::zeroed();
        page.write(7, 0xDEAD_BEEF).unwrap();
        assert_eq!(page.read(7).unwrap(), 0xDEAD_BEEF);
    }

    #[test]
    fn writes_stay_local_to_their_slots__F_A_06() {
        let mut page = ParamPage::zeroed();
        page.write(3, 1).unwrap();
        assert_eq!(page.read(4).unwrap(), 0);
    }

    #[test]
    fn out_of_range_indices_are_rejected__F_A_06() {
        let mut page = ParamPage::zeroed();
        for bad in [-1, -255, PARAM_PAGE_SLOTS as i64, 999, i64::MAX] {
            let error = page.write(bad, 1).unwrap_err();
            assert_eq!(error.index, bad);
            assert!(page.read(bad).is_err());
        }
    }

    #[test]
    fn page_error_message_names_the_range__F_A_06() {
        let error = PageError {
            index: 999,
            limit: PARAM_PAGE_SLOTS as i64,
        };
        let text = error.to_string();
        assert!(text.contains("999"), "{text}");
        assert!(text.contains("[0, 512)"), "{text}");
    }
}
