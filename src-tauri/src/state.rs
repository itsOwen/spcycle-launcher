use serde::Serialize;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Busy {
    Components,
    Downloading,
    Verifying,
    Launching,
    Playing,
    Uninstalling,
    Updating,
    Stash,
}

impl Busy {
    // reads as a noun phrase: "X is already running."
    pub fn label(self) -> &'static str {
        match self {
            Busy::Components => "Installing components",
            Busy::Downloading => "The download",
            Busy::Verifying => "Verifying",
            Busy::Launching => "Starting the game",
            Busy::Playing => "The game",
            Busy::Uninstalling => "Uninstalling",
            Busy::Updating => "The launcher update",
            Busy::Stash => "The stash editor",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Phase {
    NeedsComponents,
    NeedsGame,
    InstallingComponents,
    Downloading,
    Paused,
    Verifying,
    Ready,
    Starting,
    Playing,
    Uninstalling,
    Updating,
    Editing,
}

impl From<Busy> for Phase {
    fn from(b: Busy) -> Self {
        match b {
            Busy::Components => Phase::InstallingComponents,
            Busy::Downloading => Phase::Downloading,
            Busy::Verifying => Phase::Verifying,
            Busy::Launching => Phase::Starting,
            Busy::Playing => Phase::Playing,
            Busy::Uninstalling => Phase::Uninstalling,
            Busy::Updating => Phase::Updating,
            Busy::Stash => Phase::Editing,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceState {
    #[default]
    Down,
    Starting,
    Up,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize)]
pub struct Services {
    pub mongo: ServiceState,
    pub server: ServiceState,
    pub steam: ServiceState,
}

#[cfg(test)]
mod tests {
    use super::*;

    // the frontend keys a lookup table off these strings
    #[test]
    fn phase_serialises_as_screaming_snake_case() {
        let json = serde_json::to_string(&Phase::NeedsComponents).unwrap();
        assert_eq!(json, "\"NEEDS_COMPONENTS\"");
        assert_eq!(serde_json::to_string(&Phase::Ready).unwrap(), "\"READY\"");
    }

    #[test]
    fn service_state_serialises_lowercase() {
        assert_eq!(serde_json::to_string(&ServiceState::Up).unwrap(), "\"up\"");
        assert_eq!(
            serde_json::to_string(&Services::default()).unwrap(),
            r#"{"mongo":"down","server":"down","steam":"down"}"#
        );
    }

    // a busy that mapped to ready would let a second operation start
    #[test]
    fn every_busy_maps_to_a_running_phase() {
        for b in [
            Busy::Components,
            Busy::Downloading,
            Busy::Verifying,
            Busy::Launching,
            Busy::Playing,
            Busy::Uninstalling,
            Busy::Updating,
            Busy::Stash,
        ] {
            let phase = Phase::from(b);
            assert!(
                !matches!(
                    phase,
                    Phase::Ready | Phase::NeedsGame | Phase::NeedsComponents
                ),
                "{b:?} maps to {phase:?}, which the UI treats as idle"
            );
            assert!(!b.label().is_empty());
        }
    }
}
