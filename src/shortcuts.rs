//! Shortcut bindings: which keys and modifiers the settings accept.
//!
//! The keys themselves arrive event driven from Raw Input (`input.rs`,
//! `input_windows.rs`) and the state machine is `Gaming::advance` in the host.
//! Nothing in this file reads key state from the system, and no keyboard hook,
//! input interception, key history or process injection exists. This adapter
//! alone must never authorize transmission.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub alt: bool,
    pub control: bool,
    pub shift: bool,
    pub windows: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingError {
    UnsupportedKey,
    ReservedCombination,
}

/// Validated Win32 virtual-key binding. This is layout-dependent, not a portable
/// physical scan-code binding. Mouse 4/5 are physical buttons on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    key: u16,
    modifiers: Modifiers,
}

impl Binding {
    pub fn new(key: u16, modifiers: Modifiers) -> Result<Self, BindingError> {
        // Deliberately bounded: letters, digits, F1-F24, space, and side mouse
        // buttons. System/navigation/modifier-only keys are not accepted.
        if !(matches!(key, 0x05 | 0x06 | 0x20 | 0x30..=0x39 | 0x41..=0x5a | 0x70..=0x87)) {
            return Err(BindingError::UnsupportedKey);
        }
        if modifiers.windows || (modifiers.alt && matches!(key, 0x20 | 0x73)) {
            return Err(BindingError::ReservedCombination);
        }
        Ok(Self { key, modifiers })
    }

    pub fn key(self) -> u16 {
        self.key
    }

    pub fn modifiers(self) -> Modifiers {
        self.modifiers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_validate_keys_and_reserved_combinations() {
        for key in [0, 0x10, 0x12, 0x1b, 0x2e, 0x5b, 0x88, 256] {
            assert_eq!(
                Binding::new(key, Modifiers::default()),
                Err(BindingError::UnsupportedKey)
            );
        }
        assert!(Binding::new(0x06, Modifiers::default()).is_ok());
        for key in [0x30, 0x41, 0x70, 0x87] {
            assert!(Binding::new(key, Modifiers::default()).is_ok());
        }
        assert_eq!(
            Binding::new(
                0x4c,
                Modifiers {
                    windows: true,
                    ..Modifiers::default()
                }
            ),
            Err(BindingError::ReservedCombination)
        );
        assert_eq!(
            Binding::new(
                0x73,
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                }
            ),
            Err(BindingError::ReservedCombination)
        );
    }
}
