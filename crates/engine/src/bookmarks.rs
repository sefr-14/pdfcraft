// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! The bookmark list.

/// A bookmark as the bookmark list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct BookmarkRow {
    /// Child indices from the top level, 0-based, as the edit functions take them.
    pub path: Vec<usize>,
    pub title: String,
    /// Shown expanded (a positive `/Count`).
    pub open: bool,
    /// The 0-based page it goes to.
    pub page: Option<usize>,
    /// Where on that page it goes ([`pdfcraft_render::DestView::Top`] when it doesn't say).
    pub view: pdfcraft_render::DestView,
    /// The title's colour (`/C`), `None` for the default black, and its style (`/F`).
    pub color: Option<[f32; 3]>,
    pub italic: bool,
    pub bold: bool,
}

/// One page of the bookmark list, in outline order.
#[derive(Clone, Debug, PartialEq)]
pub struct BookmarkPage {
    pub rows: Vec<BookmarkRow>,
    /// Where the next page starts, or `None` at the end.
    pub next: Option<usize>,
    /// Set only when the document's structure could not be read for editing: its listing then
    /// comes from the viewer's copy of the outline, which is shorter than the file's.
    pub truncated: bool,
}

/// The rows of a viewer outline, in outline order, with their paths.
pub(crate) fn outline_rows(items: &[pdfcraft_render::OutlineItem]) -> Vec<BookmarkRow> {
    let mut rows = Vec::new();
    // One entry per open level: its items, and how many of them have been listed.
    let mut levels: Vec<(&[pdfcraft_render::OutlineItem], usize)> = vec![(items, 0)];
    while let Some(&(list, next)) = levels.last() {
        let Some(item) = list.get(next) else {
            levels.pop();
            continue;
        };
        if let Some(top) = levels.last_mut() {
            top.1 += 1;
        }
        let path: Vec<usize> = levels.iter().map(|(_, listed)| listed.saturating_sub(1)).collect();
        rows.push(BookmarkRow {
            path,
            title: item.title.clone(),
            open: item.open,
            page: item.page,
            view: item.view,
            color: item.color,
            italic: item.italic,
            bold: item.bold,
        });
        if !item.children.is_empty() {
            levels.push((&item.children, 0));
        }
    }
    rows
}

/// This fork (BOOKMARK.1): what indenting (`outdent: false`) or outdenting the bookmarks at
/// `paths` of the viewer's outline `items` would do, without doing it: their new paths, or why
/// the group can't move. The panel uses it to offer the command and to select the moved
/// bookmarks after it. It is a preview: `items` is the viewer's window of the outline, which
/// leaves out repeated links and what lies past its limits, so the edit itself
/// ([`crate::Edit::ShiftBookmarks`]) checks the document again and decides.
pub fn preview_shift(
    items: &[pdfcraft_render::OutlineItem],
    paths: &[Vec<usize>],
    outdent: bool,
) -> Result<Vec<Vec<usize>>, pdfcraft_organize::OutlineError> {
    let mut shape = pdfcraft_organize::OutlineShape::new();
    // Each viewer item once, with the node it hangs from; an explicit stack, no recursion.
    let mut todo: Vec<(usize, &pdfcraft_render::OutlineItem)> = items.iter().rev().map(|i| (0, i)).collect();
    while let Some((parent, item)) = todo.pop() {
        let Some(node) = shape.add(parent) else { continue };
        todo.extend(item.children.iter().rev().map(|c| (node, c)));
    }
    let shift = if outdent { pdfcraft_organize::Shift::Out } else { pdfcraft_organize::Shift::In };
    pdfcraft_organize::plan_shift(&shape, paths, shift).map(|plan| plan.paths)
}
