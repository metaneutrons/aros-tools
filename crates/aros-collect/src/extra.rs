//! Inputs which the first collector pass can prove it still needs.
//!
//! The historical collector obtains these facts by invoking `nm`.  The Rust
//! collector already owns the parsed ELF symbol table, so doing that again
//! through a host command would add both a PATH dependency and another parser.

use aros_common::elf::{Binding, Home, Symbol};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirements {
    pub cxx_pure_virtual: bool,
    pub pthread: bool,
}

#[must_use]
pub fn discover(symbols: &[Symbol]) -> Requirements {
    let mut requirements = Requirements {
        cxx_pure_virtual: false,
        pthread: false,
    };
    for symbol in symbols {
        if symbol.home != Home::Undefined {
            continue;
        }
        if symbol.name == "__cxa_pure_virtual" && symbol.binding == Binding::Weak {
            requirements.cxx_pure_virtual = true;
        }
        if symbol.name.starts_with("pthread_") && symbol.binding == Binding::Global {
            requirements.pthread = true;
        }
    }
    requirements
}

#[cfg(test)]
mod tests {
    use super::*;

    fn undefined(name: &str, binding: Binding) -> Symbol {
        Symbol {
            name: name.to_owned(),
            value: 0,
            size: 0,
            home: Home::Undefined,
            binding,
        }
    }

    #[test]
    fn reference_collector_extras_are_recognised() {
        let found = discover(&[
            undefined("__cxa_pure_virtual", Binding::Weak),
            undefined("pthread_mutex_lock", Binding::Global),
        ]);
        assert!(found.cxx_pure_virtual);
        assert!(found.pthread);
    }

    #[test]
    fn definitions_and_unrelated_weak_symbols_are_ignored() {
        let mut defined = undefined("pthread_mutex_lock", Binding::Global);
        defined.home = Home::Section(1);
        let found = discover(&[defined, undefined("some_optional_hook", Binding::Weak)]);
        assert!(!found.cxx_pure_virtual);
        assert!(!found.pthread);
    }
}
