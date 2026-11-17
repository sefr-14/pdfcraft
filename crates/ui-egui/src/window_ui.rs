// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! The state of one PdfCraft window (TAB.WINDOW.1): its tabs, panes and layout, side panels,
//! tool, keys, palette, notices and titles. [`crate::PdfCraftApp`] keeps what every window shares
//! (the session, dialogs, preferences, clipboards, background work) and holds its one window's
//! state in [`crate::PdfCraftApp::win`].

use pdfcraft_engine::DocId;

use crate::canvas::{self, DocView};
use crate::state::{LeftPanel, QuickTool, RightPanel};
use crate::{ViewLayout, hotbar, page_keys, panes, side_panels, tool_keys};

pub struct WindowUi {
    /// Split view (`panes`); which pane shows each tab is its view's.
    pub panes: panes::Panes,
    /// Tools side and pane widths; which panes are open is `left_open` and `right`.
    pub side_panels: side_panels::SidePanels,
    pub views: Vec<DocView>,
    pub(crate) closed_tabs: Vec<String>,
    /// `None` shows the Home tab.
    pub active: Option<usize>,
    /// Tab View or Tile View (OV7.2). Tab View at every start: it isn't kept in the settings.
    pub layout_mode: ViewLayout,
    /// The tiles drawn this frame in Tile View, which render alongside the active view.
    pub(crate) tile_frame: Vec<usize>,
    pub left: LeftPanel,
    pub left_open: bool,
    pub right: Option<RightPanel>,
    /// The side panel F4 reopens: the one open last.
    pub(crate) last_right: RightPanel,
    /// The right panel that was open when the user switched to Read mode, which starts with the
    /// panes closed; leaving Read mode brings it back unless the user opened or closed a pane in
    /// Read mode meanwhile.
    pub(crate) right_before_read: Option<RightPanel>,
    /// The user closed the Comments panel, so picking a comment tool leaves it closed until they
    /// open it again (#225). Remembered across restarts.
    pub comments_panel_closed: bool,
    /// The user closed the right panel, so opening a document leaves it closed instead of
    /// showing its comments or bookmarks (#1590). Remembered across restarts.
    pub right_panel_closed: bool,
    pub quick_tool: QuickTool,
    /// The tool to go back to when Space, held for a temporary Hand, is released.
    pub(crate) space_hand: Option<QuickTool>,
    /// The first key of a tool-key pair, waiting for the second (this fork, KEY.1).
    pub(crate) pending_keys: Option<tool_keys::Pending>,
    /// The tool before full screen, given back on leaving it (see
    /// [`crate::PdfCraftApp::apply_full_screen`]).
    pub(crate) full_screen_tool: Option<QuickTool>,
    pub(crate) hotbar: hotbar::Hotbar,
    pub palette_open: bool,
    pub palette_query: String,
    pub all_tools_expanded: bool,
    /// The All tools view fills the workspace (its sidebar's expand button).
    pub all_tools_full: bool,
    pub all_tools_query: String,
    /// The tab that showed when the expanded All tools view opened
    /// ([`crate::PdfCraftApp::shown_tab`]). Another tab coming to the front (Home, a document,
    /// Combine files) puts the view away.
    pub(crate) all_tools_tab: (Option<DocId>, bool),
    /// Shortcuts pressed while a text field had the keyboard, run on the next frame (see
    /// `registry_shortcuts`).
    pub(crate) deferred_commands: Vec<(&'static str, Option<canvas::ViewKey>)>,
    /// Input a menu click stands for (⌘C, Delete…), waiting to join a pass's input (see
    /// `release_menu_input`).
    pub(crate) menu_input: Vec<egui::Event>,
    /// Edit ▸ Paste clicks waiting to ask the platform for the clipboard (see
    /// `release_menu_input`).
    pub(crate) menu_pastes: usize,
    /// This frame's keys as the pages see them: none while something is over them.
    pub(crate) page_keys: page_keys::PageKeys,
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
            left: LeftPanel::AllTools,
            side_panels: Default::default(),
            left_open: true,
            right: None,
            last_right: RightPanel::Comments,
            right_before_read: None,
            comments_panel_closed: false,
            right_panel_closed: false,
            quick_tool: QuickTool::Select,
            space_hand: None,
            pending_keys: None,
            full_screen_tool: None,
            hotbar: Default::default(),
            palette_open: false,
            palette_query: String::new(),
            all_tools_expanded: false,
            all_tools_full: false,
            all_tools_query: String::new(),
            all_tools_tab: (None, false),
            deferred_commands: Vec::new(),
            menu_input: Vec::new(),
            menu_pastes: 0,
            page_keys: Default::default(),
        }
    }
}

impl Default for WindowUi {
    fn default() -> Self {
        Self::new()
    }
}
