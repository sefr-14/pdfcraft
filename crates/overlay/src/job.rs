//! A whole overlay from two documents (the wizard, step 6; later `doc_overlay`, step 5): the page
//! pairs, each with its confirmed sheet match and registration, composed into one layered
//! document with its report. It only puts steps 2 to 4 together: every rule, check and error is
//! theirs.
//!
//! Everything that can stop the overlay is found before anything is composed: every page's
//! unresolved optional content, from both documents, comes back in one
//! [`OverlayError::Unresolved`] (O12), so one run lists every decision needed.

use pdfcraft_content::Matrix;
use pdfcraft_cos::Document;

use crate::layers::{LayerStyle, LayeredOutput, RegistrationNote, Roles};
use crate::oc::prepare::{Prepared, PreparedSource, prepare};
use crate::oc::{Unresolved, Visibility};
use crate::report::OverlayReport;
use crate::{DisplayFrame, OverlayError, Placement, Settings, SheetMatch, Warning, frame_of_page, place};

/// One output page: a page of A and a page of B (0-based; `None` is a blank side, never both),
/// B's confirmed sheet match and its registration (B's displayed frame after the sheet match →
/// A's displayed frame; the identity when there is none).
#[derive(Clone, Debug, PartialEq)]
pub struct PairPlan {
    pub a: Option<usize>,
    pub b: Option<usize>,
    pub sheet: SheetMatch,
    pub registration: Matrix,
    pub note: RegistrationNote,
}

impl PairPlan {
    /// A pair placed 1:1 with no registration.
    pub fn placed(a: Option<usize>, b: Option<usize>) -> Self {
        PairPlan { a, b, sheet: SheetMatch::Off, registration: Matrix::IDENTITY, note: RegistrationNote::PLACED }
    }
}

/// Everything the overlay needs besides the two documents and their visibility.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub pairs: Vec<PairPlan>,
    pub style: LayerStyle,
    pub roles: Roles,
    /// The wizard's "Expand page to prevent clipping" (until OV6.8 "Expand output to include both
    /// documents"): on, each page is the union of both placed sheets; off keeps A's sheet and
    /// clips B. It never changes the number of pages.
    pub expand: bool,
    pub settings: Settings,
}

/// A finished overlay.
#[derive(Debug)]
pub struct Generated {
    pub doc: Document,
    pub report: OverlayReport,
    /// Frame warnings (pdfcraft's own fallbacks for odd page boxes), each naming its page.
    pub warnings: Vec<Warning>,
    /// Output pages (0-based) where part of B falls outside A's sheet and is clipped.
    pub clipped: Vec<usize>,
}

/// The displayed frames of a pair, with a blank side taking the other's frame: the output page is
/// A's sheet, or B's when A is blank.
pub fn pair_frames(a: &Document, b: &Document, pair: &PairPlan) -> Result<(DisplayFrame, DisplayFrame, Vec<Warning>), OverlayError> {
    let mut warnings = Vec::new();
    let mut frame = |doc: &Document, page: Option<usize>| -> Result<Option<DisplayFrame>, OverlayError> {
        let Some(page) = page else { return Ok(None) };
        let (f, w) = frame_of_page(doc, page)?;
        warnings.extend(w);
        Ok(Some(f))
    };
    let (fa, fb) = (frame(a, pair.a)?, frame(b, pair.b)?);
    match (fa, fb) {
        (Some(fa), Some(fb)) => Ok((fa, fb, warnings)),
        (Some(f), None) | (None, Some(f)) => Ok((f, f, warnings)),
        (None, None) => Err(OverlayError::NoSources),
    }
}

/// Where a pair's documents go on its output page. A blank side places the other 1:1.
pub fn pair_placement(
    a: &Document,
    b: &Document,
    pair: &PairPlan,
    expand: bool,
    settings: &Settings,
) -> Result<(Placement, Vec<Warning>), OverlayError> {
    let (fa, fb, warnings) = pair_frames(a, b, pair)?;
    let both = pair.a.is_some() && pair.b.is_some();
    let (sheet, registration) = if both { (pair.sheet, pair.registration) } else { (SheetMatch::Off, Matrix::IDENTITY) };
    Ok((place(&fa, &fb, sheet, &registration, expand, settings)?, warnings))
}

/// Every unresolved item of the pages the plan uses, side by side, each page once.
fn unresolved(prepared: &Option<Prepared>, pages: &[usize]) -> Result<Vec<Unresolved>, OverlayError> {
    let Some(p) = prepared else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for &page in pages {
        out.extend(p.source(page)?.unresolved().iter().cloned());
    }
    Ok(out)
}

fn prepared_source(p: &Option<Prepared>, page: Option<usize>) -> Result<Option<PreparedSource<'_>>, OverlayError> {
    match (p, page) {
        (Some(p), Some(page)) => p.source(page).map(Some),
        _ => Ok(None),
    }
}

fn used(pairs: &[PairPlan], side: impl Fn(&PairPlan) -> Option<usize>) -> Vec<usize> {
    let mut pages: Vec<usize> = pairs.iter().filter_map(side).collect();
    pages.sort_unstable();
    pages.dedup();
    pages
}

/// Prepare both documents for the plan's pages under their visibility, check every page, then
/// compose the overlay. Nothing is returned, and nothing composed, unless every page resolves.
/// The sources are only read.
pub fn generate(a: &Document, b: &Document, visibility_a: &Visibility, visibility_b: &Visibility, plan: &Plan) -> Result<Generated, OverlayError> {
    if plan.pairs.is_empty() || plan.pairs.iter().any(|p| p.a.is_none() && p.b.is_none()) {
        return Err(OverlayError::NoSources);
    }
    let (pages_a, pages_b) = (used(&plan.pairs, |p| p.a), used(&plan.pairs, |p| p.b));
    let prepared_a = if pages_a.is_empty() { None } else { Some(prepare(a, &pages_a, visibility_a)?) };
    let prepared_b = if pages_b.is_empty() { None } else { Some(prepare(b, &pages_b, visibility_b)?) };
    let (ua, ub) = (unresolved(&prepared_a, &pages_a)?, unresolved(&prepared_b, &pages_b)?);
    if !ua.is_empty() || !ub.is_empty() {
        return Err(OverlayError::Unresolved { a: ua, b: ub, roles: plan.roles });
    }
    let mut out = LayeredOutput::new()?;
    let mut warnings = Vec::new();
    let mut clipped = Vec::new();
    for (i, pair) in plan.pairs.iter().enumerate() {
        let (placement, w) = pair_placement(a, b, pair, plan.expand, &plan.settings)?;
        warnings.extend(w);
        if placement.b_clipped && pair.a.is_some() && pair.b.is_some() {
            clipped.push(i);
        }
        let (sa, sb) = (prepared_source(&prepared_a, pair.a)?, prepared_source(&prepared_b, pair.b)?);
        let note = if pair.a.is_some() && pair.b.is_some() { pair.note } else { RegistrationNote::PLACED };
        out.add_page(sa, sb, &placement, &plan.style, plan.roles, note)?;
    }
    let (doc, report) = out.finish_with_report()?;
    Ok(Generated { doc, report, warnings, clipped })
}

/// The explicit decisions that make the overlay show a source's layers as its tab shows them (the
/// visibility snapshot, main PRD decision 1b): `shown` is each layer's state in the tab. A layer
/// whose state matches what the file's default configuration resolves to needs no decision. A
/// layer the default configuration can't resolve gets none either: it stays unresolved, so the
/// wizard asks, with the tab's state as the suggestion, rather than settling it silently.
pub fn snapshot_decisions(doc: &Document, shown: &[(pdfcraft_cos::ObjRef, bool)]) -> Vec<(pdfcraft_cos::ObjRef, bool)> {
    let mut ev = crate::oc::Evaluator::new(doc, &Visibility::default());
    let mut out = Vec::new();
    for &(group, on) in shown {
        if !ev.is_optional_content(group) {
            continue;
        }
        match ev.visibility(&pdfcraft_cos::Object::Ref(group)).visible {
            Some(v) if v == on => {}
            Some(_) => out.push((group, on)),
            None => {}
        }
    }
    out
}
