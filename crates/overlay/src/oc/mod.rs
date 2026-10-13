//! A source's optional content (step 4): what each reference shows under the file's default
//! configuration and any explicit decisions (OV4.2, [`eval`]), and the rewriting that removes
//! hidden content from an in-memory copy (OV4.3).
//!
//! Every rule is either decided by the standard, so the output is demonstrably correct, or the
//! item is reported as unresolved. Nothing falls back to a guess, and an unresolved item that
//! can't change the result (an AnyOn with another member visible, say) doesn't block it (step 4
//! PRD, Resolving optional content).

pub mod eval;
pub mod prepare;
pub(crate) mod resolve;

use std::fmt;

use pdfcraft_cos::ObjRef;

pub use eval::{Evaluator, Note, Problem, Verdict};

/// Explicit visibility decisions (O5): optional content groups, membership dictionaries or other
/// objects a file uses as optional content, each set on (`true`) or off. Empty: the file's
/// default configuration alone.
///
/// `omit` names annotations the caller accepts leaving out of the overlay although the source
/// shows them (step 4: an appearance the overlay can't reproduce faithfully, O15). Without that
/// decision such an annotation leaves its page unresolved; with it, the report lists it as
/// omitted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Visibility {
    pub decisions: Vec<(ObjRef, bool)>,
    pub omit: Vec<ObjRef>,
}

/// Why an optional-content reference can't be resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Rule {
    /// The default configuration's `/BaseState` isn't ON or OFF (Unchanged included).
    BaseState,
    /// The default configuration's `/ON`, `/OFF`, `/OCGs` or `/Intent` isn't what the standard
    /// allows, so no group's default state can be trusted.
    MalformedConfiguration,
    /// A group is listed in both `/ON` and `/OFF`.
    OnAndOff,
    /// A group's `/Intent` isn't a name or an array of names.
    MalformedIntent,
    /// A group's intents don't match the default configuration's: the standard leaves it out of
    /// visibility, but pdfcraft's display uses its state (O13).
    IntentMismatch,
    /// A group isn't listed in the catalog's `/OCGs`.
    NotInOcgs,
    /// A View auto-state would change a group's state; the clause can't be confirmed (O5).
    AutoStateView,
    /// An auto-state that depends on the viewer (zoom, language, user and the like) applies to
    /// the group.
    AutoStateEnvironment,
    /// An auto-state entry isn't what the standard allows.
    MalformedAutoState,
    /// A visibility expression with an unknown operator, a wrong operand count, a leaf that isn't
    /// a group, a cycle, or nesting deeper than 32.
    MalformedExpression,
    /// A membership dictionary's visibility expression (`/VE`) and its `/OCGs` with `/P` give
    /// different results. ISO 32000-1 prefers `/VE` only in an informative note (§8.11.2.2,
    /// NOTE 2), and pdfcraft's display reads `/OCGs` and `/P`, so neither is followed (OV4.7, F1).
    ExpressionDisagrees,
    /// A membership dictionary's `/OCGs` names deleted objects, and the result depends on them:
    /// the standard ignores them (Table 99), pdfcraft's display counts them as groups that are on
    /// (OV4.7, F2).
    DeletedMember,
    /// A membership dictionary's `/P` isn't AllOn, AnyOn, AnyOff or AllOff.
    UnknownPolicy,
    /// A membership dictionary's `/OCGs` isn't a group or an array of groups.
    MalformedMembership,
    /// An optional-content reference that isn't a group or a membership dictionary.
    NotOptionalContent,
    /// An inline dictionary after `/OC … BDC`, or a group written directly instead of by
    /// reference, which nothing can name.
    Inline,
    /// Nested past the limit, or a cycle, while following references.
    TooDeep,
    /// A marker names a property the stream's resources don't define.
    UnknownProperty,
    /// A content stream that has to be checked or rewritten can't be decoded, or the parser
    /// skipped bytes in it.
    Unreadable,
    /// Marked content that doesn't balance (an end without a beginning, or a beginning never
    /// ended) where optional content is involved: viewers close these differently.
    Unbalanced,
    /// A hidden block whose graphics-state saves and restores, or text object, cross its
    /// boundary.
    HiddenBlockCrossesStructure,
    /// Hidden text that changes where visible text goes: inside a text object that visible text
    /// after it continues from, or setting spacing or leading (`"`, `TD`) that later text uses.
    HiddenTextMovesVisibleText,
    /// Hidden text that may be in a clipping render mode, with content drawn after it: viewers
    /// differ on whether hidden text clips.
    HiddenClippingText,
    /// A form without its own resources is drawn from places with different resources, so its
    /// layer markers can't be resolved once for all of them.
    AmbiguousResources,
    /// Optional content referenced from data the overlay copies but doesn't draw (private
    /// application data, for example), so the output's two layers can't be the only ones.
    InResources,
    /// An annotation whose stored appearance PdfCraft drew with `?` in place of characters its
    /// fonts can't draw (the PCUndrawn mark, O15): drawing it would show `?` as if it were the
    /// text.
    AnnotationUndrawnText,
    /// An annotation without a usable appearance whose text the standard fonts (or a form field's
    /// font) can't draw: generating one would show `?` in its place.
    AnnotationUndrawableText,
    /// An annotation the overlay doesn't draw (3D, RichMedia, Screen, Movie, Sound) that has an
    /// appearance the source shows.
    AnnotationNotDrawn,
    /// The page's content leaves graphics states saved or a text object open at its end, or
    /// restores more than it saves, so its annotations can't be placed where viewers place them.
    AnnotationPlacement,
}

impl Rule {
    pub fn describe(self) -> &'static str {
        match self {
            Rule::BaseState => "the default layer configuration's base state isn't ON or OFF",
            Rule::MalformedConfiguration => "the default layer configuration isn't valid",
            Rule::OnAndOff => "a layer is listed both as on and as off",
            Rule::MalformedIntent => "a layer's intent isn't valid",
            Rule::IntentMismatch => "a layer's intent doesn't match the configuration's (Design layers, for example)",
            Rule::NotInOcgs => "a layer isn't declared in the document's layer list",
            Rule::AutoStateView => "an automatic View state would change a layer",
            Rule::AutoStateEnvironment => "a layer's visibility depends on the viewer (zoom, language or user)",
            Rule::MalformedAutoState => "an automatic layer state isn't valid",
            Rule::MalformedExpression => "a visibility expression isn't valid",
            Rule::ExpressionDisagrees => "a layer's visibility expression and its layer list give different results",
            Rule::DeletedMember => "a layer membership names a deleted layer, and viewers disagree on what it shows",
            Rule::UnknownPolicy => "a layer policy isn't AllOn, AnyOn, AnyOff or AllOff",
            Rule::MalformedMembership => "a layer membership list isn't valid",
            Rule::NotOptionalContent => "content is marked with something that isn't a layer",
            Rule::Inline => "a layer is written inline, so nothing can name it",
            Rule::TooDeep => "layer references nest too deeply or loop",
            Rule::UnknownProperty => "content is marked with a layer name the page doesn't define",
            Rule::Unreadable => "a content stream with layers can't be read completely",
            Rule::Unbalanced => "layer markers don't balance",
            Rule::HiddenBlockCrossesStructure => "hidden content starts or ends inside a graphics-state group or text object",
            Rule::HiddenTextMovesVisibleText => "hidden text changes where visible text goes",
            Rule::HiddenClippingText => "hidden text may clip what is drawn after it",
            Rule::AmbiguousResources => "a drawing without its own resources is used in places with different resources",
            Rule::InResources => "layers are referenced from data the page carries but doesn't draw",
            Rule::AnnotationUndrawnText => "an annotation's appearance shows ? in place of characters PdfCraft's fonts couldn't draw",
            Rule::AnnotationUndrawableText => "an annotation has text the fonts can't draw, so its appearance would show ?",
            Rule::AnnotationNotDrawn => "an annotation of a kind the overlay doesn't draw (3D, media, sound) has a visible appearance",
            Rule::AnnotationPlacement => "the page's content doesn't end in its initial graphics state, so its annotations can't be placed reliably",
        }
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.describe())
    }
}

/// One unresolved item of a source page: the objects involved (optional-content objects by
/// reference, with the group's name where it has one; an annotation by reference, with its kind
/// and position), the rule that failed, whether an explicit decision on one of those objects
/// settles it (a layer decision, or omitting the annotation), and any detail (the character that
/// can't be drawn, say).
#[derive(Clone, Debug, PartialEq)]
pub struct Unresolved {
    /// 0-based.
    pub page: usize,
    pub objects: Vec<(ObjRef, Option<String>)>,
    pub rule: Rule,
    pub settleable: bool,
    pub detail: Option<String>,
}

impl fmt::Display for Unresolved {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "page {}: {}", self.page.saturating_add(1), self.rule)?;
        let names: Vec<String> = self
            .objects
            .iter()
            .map(|(r, n)| match n {
                Some(n) => format!("\"{n}\" ({} {} R)", r.num, r.generation),
                None => format!("{} {} R", r.num, r.generation),
            })
            .collect();
        if !names.is_empty() {
            write!(f, " [{}]", names.join(", "))?;
        }
        if let Some(d) = &self.detail {
            write!(f, " ({d})")?;
        }
        if self.settleable {
            let annotation = matches!(
                self.rule,
                Rule::AnnotationUndrawnText | Rule::AnnotationUndrawableText | Rule::AnnotationNotDrawn | Rule::AnnotationPlacement
            );
            write!(
                f,
                "; {}",
                if annotation { "an explicit decision to omit the annotation settles it" } else { "an explicit layer decision settles it" }
            )?;
        }
        Ok(())
    }
}
