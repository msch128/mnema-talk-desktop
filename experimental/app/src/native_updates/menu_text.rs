//! The de/en texts of the update entries of the app menu. The host has no
//! locale of its own: the rail page reports the language it shows.

pub(super) struct MenuText {
    /// The submenu's title while nothing waits.
    pub(super) idle: &'static str,
    /// The title while an update waits; the dot marks it.
    pub(super) offered: &'static str,
    pub(super) now: &'static str,
    pub(super) check: &'static str,
}

pub(super) const EN: MenuText = MenuText {
    idle: "Updates",
    offered: "Update available •",
    now: "Update now",
    check: "Check for updates…",
};

pub(super) const DE: MenuText = MenuText {
    idle: "Updates",
    offered: "Update verfügbar •",
    now: "Jetzt aktualisieren",
    check: "Nach Updates suchen …",
};
