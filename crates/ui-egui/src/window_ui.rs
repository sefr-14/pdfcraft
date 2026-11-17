// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! The state of one PdfCraft window (TAB.WINDOW.1): its tabs, panes and layout, side panels,
//! tool, keys, palette, notices and titles. [`crate::PdfCraftApp`] keeps what every window shares
//! (the session, dialogs, preferences, clipboards, background work) and holds its one window's
//! state in [`crate::PdfCraftApp::win`].

use crate::canvas::DocView;
use crate::{ViewLayout, panes};

pub struct WindowUi {
    /// Split view (`panes`); which pane shows each tab is its view's.
    pub panes: panes::Panes,
    pub views: Vec<DocView>,
    pub(crate) closed_tabs: Vec<String>,
    /// `None` shows the Home tab.
    pub active: Option<usize>,
    /// Tab View or Tile View (OV7.2). Tab View at every start: it isn't kept in the settings.
    pub layout_mode: ViewLayout,
    /// The tiles drawn this frame in Tile View, which render alongside the active view.
    pub(crate) tile_frame: Vec<usize>,
}

impl WindowUi {
    /// A window with no tabs, showing Home, as PdfCraft starts.
    pub fn new() -> Self {
        Self {
            panes: Default::default(),
            views: Vec::new(),
            closed_tabs: Vec::new(),
            active: None,
            layout_mode: ViewLayout::Tabs,
            tile_frame: Vec::new(),
        }
    }
}

impl Default for WindowUi {
    fn default() -> Self {
        Self::new()
    }
}
