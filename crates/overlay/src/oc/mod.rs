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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Visibility {
    pub decisions: Vec<(ObjRef, bool)>,
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
        }
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.describe())
    }
}

/// One unresolved item of a source page: the optional-content objects involved (by reference,
/// with the group's name where it has one), the rule that failed, and whether an explicit
/// decision on one of those objects settles it.
#[derive(Clone, Debug, PartialEq)]
pub struct Unresolved {
    /// 0-based.
    pub page: usize,
    pub objects: Vec<(ObjRef, Option<String>)>,
    pub rule: Rule,
    pub settleable: bool,
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
        if self.settleable {
            write!(f, "; an explicit layer decision settles it")?;
        }
        Ok(())
    }
}
