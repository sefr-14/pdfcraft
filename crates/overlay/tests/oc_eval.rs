//! The optional-content evaluator (OV4.2): a test for every row of the step 4 PRD's Resolving
//! tables, every auto-state case, explicit decisions, and a 10,000-case hostile sweep.

mod common;

use std::time::Instant;

use common::Rng;
use common::synth;
use pdfcraft_cos::{Document, ObjRef, Object};
use pdfcraft_overlay::Rule;
use pdfcraft_overlay::oc::{Evaluator, Note, Verdict, Visibility};

/// A file whose catalog has `props` (`/OCProperties …` or nothing) and whose objects 5, 6, … are
/// `objects`.
fn file(props: &str, objects: &[&str]) -> Document {
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {props} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".to_string(),
        synth::raw_stream("", "0 g 0 0 1 1 re f"),
    ];
    objs.extend(objects.iter().map(|s| s.to_string()));
    synth::open(&synth::raw_pdf(&objs))
}

fn r(n: u32) -> ObjRef {
    ObjRef::new(n, 0)
}

fn group(name: &str) -> String {
    format!("<< /Type /OCG /Name ({name}) >>")
}

/// The usual configuration: groups 5 (on) and 6 (off).
const TWO: &str = "/OCProperties << /OCGs [5 0 R 6 0 R] /D << /OFF [6 0 R] >> >>";

fn verdict(doc: &Document, decisions: &[(u32, bool)], o: Object) -> Verdict {
    let v = Visibility { decisions: decisions.iter().map(|(n, b)| (r(*n), *b)).collect(), ..Visibility::default() };
    Evaluator::new(doc, &v).visibility(&o)
}

fn of(doc: &Document, n: u32) -> Verdict {
    verdict(doc, &[], Object::Ref(r(n)))
}

#[track_caller]
fn visible(v: &Verdict, want: bool) {
    assert_eq!(v.visible, Some(want), "{v:?}");
    assert!(v.problems.is_empty(), "{v:?}");
}

/// Unresolved with exactly these rules, each settleable or not as given.
#[track_caller]
fn unresolved(v: &Verdict, want: &[(Rule, bool)]) {
    assert_eq!(v.visible, None, "{v:?}");
    let mut got: Vec<(Rule, bool)> = v.problems.iter().map(|p| (p.rule, p.settleable)).collect();
    got.sort();
    let mut want = want.to_vec();
    want.sort();
    assert_eq!(got, want, "{v:?}");
}

// ── Groups ──────────────────────────────────────────────────────────────────────────────────

/// Row 1: a listed group with no intent shows its state.
#[test]
fn a_listed_group_shows_its_state() {
    let doc = file(TWO, &[&group("On"), &group("Off")]);
    visible(&of(&doc, 5), true);
    visible(&of(&doc, 6), false);
}

/// `/BaseState` OFF isn't allowed in the default configuration (PDF Reference 1.7, Table 4.51);
/// the overlay follows it, as viewers do, and the report says so.
#[test]
fn base_state_on_and_off_lists() {
    let doc = file("/OCProperties << /OCGs [5 0 R 6 0 R 7 0 R] /D << /BaseState /OFF /ON [6 0 R] >> >>", &[&group("a"), &group("b"), &group("c")]);
    assert_eq!(Evaluator::new(&doc, &Visibility::default()).notes(), [Note::BaseStateOff]);
    visible(&of(&doc, 5), false);
    visible(&of(&doc, 6), true);
    visible(&of(&doc, 7), false);
    let doc = file("/OCProperties << /OCGs [5 0 R 6 0 R] /D << /BaseState /ON /OFF [6 0 R] /ON [6 0 R] >> >>", &[&group("a"), &group("b")]);
    visible(&of(&doc, 5), true);
    unresolved(&of(&doc, 6), &[(Rule::OnAndOff, true)]);
    visible(&verdict(&doc, &[(6, false)], Object::Ref(r(6))), false);
}

/// Rows 2 and 3: intents are matched with the configuration's, not with View alone.
#[test]
fn intents_are_matched_with_the_configuration() {
    let objs = [
        &group("plain")[..],
        "<< /Type /OCG /Name (view) /Intent /View >>",
        "<< /Type /OCG /Name (design) /Intent /Design >>",
        "<< /Type /OCG /Name (both) /Intent [/View /Design] >>",
        "<< /Type /OCG /Name (custom) /Intent /Engineering >>",
    ];
    let props = |intent: &str| format!("/OCProperties << /OCGs [5 0 R 6 0 R 7 0 R 8 0 R 9 0 R] /D << {intent} >> >>");
    // No configuration intent: View.
    let doc = file(&props(""), &objs);
    for n in [5, 6, 8] {
        visible(&of(&doc, n), true);
    }
    for n in [7, 9] {
        unresolved(&of(&doc, n), &[(Rule::IntentMismatch, true)]);
        visible(&verdict(&doc, &[(n, false)], Object::Ref(r(n))), false);
    }
    // Design only: the View groups don't match now.
    let doc = file(&props("/Intent /Design"), &objs);
    for n in [5, 6, 9] {
        unresolved(&of(&doc, n), &[(Rule::IntentMismatch, true)]);
    }
    for n in [7, 8] {
        visible(&of(&doc, n), true);
    }
    // View and Design.
    let doc = file(&props("/Intent [/View /Design]"), &objs);
    for n in [5, 6, 7, 8] {
        visible(&of(&doc, n), true);
    }
    unresolved(&of(&doc, 9), &[(Rule::IntentMismatch, true)]);
    // All: every group takes part.
    let doc = file(&props("/Intent /All"), &objs);
    for n in 5..=9 {
        visible(&of(&doc, n), true);
    }
}

/// Row 4: an intent that isn't a name or an array of names.
#[test]
fn malformed_intents() {
    let doc = file(
        "/OCProperties << /OCGs [5 0 R 6 0 R] /D << >> >>",
        &["<< /Type /OCG /Name (x) /Intent 5 >>", "<< /Type /OCG /Name (y) /Intent [/View 3] >>"],
    );
    unresolved(&of(&doc, 5), &[(Rule::MalformedIntent, true)]);
    unresolved(&of(&doc, 6), &[(Rule::MalformedIntent, true)]);
    visible(&verdict(&doc, &[(5, true)], Object::Ref(r(5))), true);
    // The configuration's own intent malformed: nothing can be trusted, and no decision helps.
    let doc = file("/OCProperties << /OCGs [5 0 R] /D << /Intent (View) >> >>", &[&group("x")]);
    unresolved(&of(&doc, 5), &[(Rule::MalformedConfiguration, false)]);
    unresolved(&verdict(&doc, &[(5, true)], Object::Ref(r(5))), &[(Rule::MalformedConfiguration, false)]);
}

/// Row 5: a base state other than ON or OFF, Unchanged included.
#[test]
fn base_state_unchanged_is_unresolved() {
    for base in ["/Unchanged", "/Bogus", "(ON)", "1"] {
        let doc = file(&format!("/OCProperties << /OCGs [5 0 R] /D << /BaseState {base} >> >>"), &[&group("x")]);
        unresolved(&of(&doc, 5), &[(Rule::BaseState, false)]);
        unresolved(&verdict(&doc, &[(5, true)], Object::Ref(r(5))), &[(Rule::BaseState, false)]);
    }
}

/// The configuration's lists must be what the standard allows.
#[test]
fn malformed_configurations() {
    for props in [
        "/OCProperties 5",
        "/OCProperties << /OCGs 5 0 R /D << >> >>",
        "/OCProperties << /OCGs [5 0 R 7] /D << >> >>",
        "/OCProperties << /OCGs [5 0 R] >>",
        "/OCProperties << /OCGs [5 0 R] /D << /OFF 6 >> >>",
        "/OCProperties << /OCGs [5 0 R] /D << /ON [(x)] >> >>",
    ] {
        let doc = file(props, &[&group("x")]);
        unresolved(&of(&doc, 5), &[(Rule::MalformedConfiguration, false)]);
    }
}

/// Row 6, revised by OV4.10: a group missing from `/OCGs` is shown as the source shows it. A file
/// with no optional content properties at all has readers ignore optional content (ISO 32000-1
/// §8.11.4.2) (U1); a listed configuration that leaves the group out shows it in PdfCraft's display
/// and Poppler 24.02 alike (U2, U3), whatever `/BaseState` says (U5). Where the default
/// configuration's `/OFF` names it (U4: PdfCraft hides it, Poppler shows it) or an auto-state does
/// (U6), it stays unresolved, and a decision settles it.
#[test]
fn a_group_not_listed_is_shown_as_the_source_shows_it() {
    let missing = |d: &str| file(&format!("/OCProperties << /OCGs [5 0 R] /D << {d} >> >>"), &[&group("listed"), &group("missing")]);
    // U1: no /OCProperties.
    let doc = file("", &[&group("orphan")]);
    visible(&of(&doc, 5), true);
    // U2: listed configuration, the group in neither /ON nor /OFF.
    visible(&of(&missing(""), 6), true);
    // U3: in /ON only.
    visible(&of(&missing("/ON [6 0 R]"), 6), true);
    // U5: /BaseState /OFF applies to listed groups, not to this one (the listed one is hidden).
    let doc = missing("/BaseState /OFF");
    visible(&of(&doc, 6), true);
    visible(&of(&doc, 5), false);
    // U4: named by /OFF, alone, with /BaseState /OFF, and in both /ON and /OFF.
    for d in ["/OFF [6 0 R]", "/BaseState /OFF /OFF [6 0 R]", "/ON [6 0 R] /OFF [6 0 R]"] {
        let doc = missing(d);
        unresolved(&of(&doc, 6), &[(Rule::NotInOcgs, true)]);
        visible(&verdict(&doc, &[(6, false)], Object::Ref(r(6))), false);
        visible(&verdict(&doc, &[(6, true)], Object::Ref(r(6))), true);
    }
    // U6: named by an auto-state (View or Print), even one that wouldn't change a listed group.
    for event in ["/View /Category [/View]", "/Print /Category [/Print]"] {
        let doc = missing(&format!("/AS [<< /Event {event} /OCGs [6 0 R] >>]"));
        unresolved(&of(&doc, 6), &[(Rule::NotInOcgs, true)]);
    }
    // U7: a configuration problem holds every group, listed or not.
    let doc = file("/OCProperties << /OCGs [5 0 R] /D << /BaseState /Unchanged >> >>", &[&group("listed"), &group("missing")]);
    unresolved(&of(&doc, 6), &[(Rule::BaseState, false)]);
    // An undeclared group's intent doesn't matter: both renderers ignore it, and the standard's
    // intent rule would only leave the group out of visibility.
    let doc = file("/OCProperties << /OCGs [5 0 R] /D << >> >>", &[&group("listed"), "<< /Type /OCG /Name (design) /Intent /Design >>"]);
    visible(&of(&doc, 6), true);
}

/// The report names the undeclared groups shown, once each and in one note, saying whether the
/// document has a layer configuration; groups decided, listed or unresolved aren't in it.
#[test]
fn undeclared_groups_shown_are_noted_once() {
    let doc = file(
        "/OCProperties << /OCGs [5 0 R] /D << /OFF [8 0 R] >> >>",
        &[&group("listed"), &group("a"), &group("b"), &group("off"), &group("decided")],
    );
    let mut ev = Evaluator::new(&doc, &Visibility { decisions: vec![(r(9), true)], ..Visibility::default() });
    for n in [5, 6, 7, 6, 8, 9, 7] {
        ev.visibility(&Object::Ref(r(n)));
    }
    assert_eq!(ev.notes(), [Note::UndeclaredShown { groups: vec![r(6), r(7)], configured: true }]);
    let doc = file("", &[&group("a")]);
    let mut ev = Evaluator::new(&doc, &Visibility::default());
    ev.visibility(&Object::Ref(r(5)));
    assert_eq!(ev.notes(), [Note::UndeclaredShown { groups: vec![r(5)], configured: false }]);
}

/// Membership dictionaries over undeclared groups: the groups count as shown (U2), or as they
/// are decided. Without `/OCProperties` the standard shows the content whatever the dictionary
/// says, so the dictionary is shown when PdfCraft's display (`/OCGs` and `/P` with the groups on,
/// `/VE` ignored) shows it too, and unresolved when the display would hide it (U8); a decision on
/// the dictionary settles it.
#[test]
fn memberships_over_undeclared_groups() {
    let listed = "/OCProperties << /OCGs [5 0 R] /D << /OFF [5 0 R] >> >>";
    for (p, want) in [("/P /AnyOn", true), ("/P /AllOn", false), ("/P /AnyOff", true), ("/P /AllOff", false)] {
        // 5 is listed and off, 6 is undeclared and shown.
        let doc = file(listed, &[&group("off"), &group("loose"), &format!("<< /Type /OCMD /OCGs [5 0 R 6 0 R] {p} >>")]);
        visible(&of(&doc, 7), want);
    }
    let doc = file(listed, &[&group("off"), &group("loose"), "<< /Type /OCMD /VE [/And 6 0 R [/Not 5 0 R]] >>"]);
    visible(&of(&doc, 7), true);
    // U8: no /OCProperties.
    for (ocmd, want) in [
        ("<< /Type /OCMD /OCGs [5 0 R 6 0 R] >>", Some(true)),
        ("<< /Type /OCMD /OCGs [5 0 R 6 0 R] /P /AllOn >>", Some(true)),
        ("<< /Type /OCMD /OCGs [5 0 R] /P /AllOff >>", None),
        ("<< /Type /OCMD /OCGs [5 0 R 6 0 R] /P /AnyOff >>", None),
        // /VE is ignored with the rest of optional content; the display ignores it too.
        ("<< /Type /OCMD /VE [/Not 5 0 R] >>", Some(true)),
        ("<< /Type /OCMD /OCGs [5 0 R] /VE [/Not 5 0 R] >>", Some(true)),
    ] {
        let doc = file("", &[&group("a"), &group("b"), ocmd]);
        match want {
            Some(w) => visible(&of(&doc, 7), w),
            None => {
                unresolved(&of(&doc, 7), &[(Rule::NotInOcgs, true)]);
                visible(&verdict(&doc, &[(7, true)], Object::Ref(r(7))), true);
            }
        }
    }
}

// ── Membership dictionaries ─────────────────────────────────────────────────────────────────

/// Rows 9 and 10: `/P` over `/OCGs`, AnyOn by default; a single group counts as one.
#[test]
fn policies() {
    let on_off = |p: &str| format!("<< /Type /OCMD /OCGs [5 0 R 6 0 R] {p} >>");
    let cases = [("", true), ("/P /AnyOn", true), ("/P /AllOn", false), ("/P /AnyOff", true), ("/P /AllOff", false)];
    for (p, want) in cases {
        let doc = file(TWO, &[&group("on"), &group("off"), &on_off(p)]);
        visible(&of(&doc, 7), want);
    }
    let doc = file(TWO, &[&group("on"), &group("off"), "<< /Type /OCMD /OCGs 6 0 R /P /AllOff >>"]);
    visible(&of(&doc, 7), true);
    let doc = file(TWO, &[&group("on"), &group("off"), "<< /Type /OCMD /OCGs [5 0 R] /P /AllOff >>"]);
    visible(&of(&doc, 7), false);
}

/// Row 11: an unknown policy.
#[test]
fn an_unknown_policy_is_unresolved() {
    for p in ["/P /Majority", "/P (AnyOn)", "/P 1"] {
        let doc = file(TWO, &[&group("on"), &group("off"), &format!("<< /Type /OCMD /OCGs [5 0 R] {p} >>")]);
        unresolved(&of(&doc, 7), &[(Rule::UnknownPolicy, true)]);
        visible(&verdict(&doc, &[(7, true)], Object::Ref(r(7))), true);
    }
}

/// Row 12: `/OCGs` absent, null or empty, without `/VE`: no effect on visibility (ISO 32000-1
/// Table 99, recorded as a deviation from the PRD's "unresolved until confirmed"), so the content
/// is shown, and the report says why. Nulls among real groups are ignored.
#[test]
fn an_empty_membership_has_no_effect() {
    for ocgs in ["", "/OCGs null", "/OCGs []", "/OCGs [null]"] {
        let doc = file(TWO, &[&group("on"), &group("off"), &format!("<< /Type /OCMD {ocgs} /P /AllOff >>")]);
        let mut ev = Evaluator::new(&doc, &Visibility::default());
        visible(&ev.visibility(&Object::Ref(r(7))), true);
        assert_eq!(ev.notes(), [Note::MembershipNoEffect { membership: Some(r(7)) }], "{ocgs}");
    }
    let doc = file(TWO, &[&group("on"), &group("off"), "<< /Type /OCMD /OCGs [null 6 0 R] /P /AllOn >>"]);
    visible(&of(&doc, 7), false);
}

/// OV4.7, F2: references to deleted objects in `/OCGs`. The standard ignores them (Table 99);
/// pdfcraft's display counts them as groups that are on. Where the two readings agree the result
/// stands; where they differ the dictionary is unresolved, and a decision on it settles it.
#[test]
fn deleted_members_that_change_the_result_are_unresolved() {
    // 5 on, 6 off; 99 doesn't exist.
    for (ocgs, p, want) in [
        // Only deleted members: no effect (shown), against the display's reading.
        ("[null 99 0 R]", "/AnyOn", Some(true)),
        ("[99 0 R]", "/AllOn", Some(true)),
        ("99 0 R", "/AnyOn", Some(true)),
        ("[null 99 0 R]", "/AllOff", None),
        ("99 0 R", "/AllOff", None),
        ("[99 0 R]", "/AnyOff", None),
        // Deleted members beside real groups.
        ("[null 6 0 R 99 0 R]", "/AllOn", Some(false)),
        ("[5 0 R 99 0 R]", "/AllOff", Some(false)),
        ("[5 0 R 99 0 R]", "/AnyOn", Some(true)),
        ("[6 0 R 99 0 R]", "/AllOff", None),
        ("[6 0 R 99 0 R]", "/AnyOn", None),
    ] {
        let doc = file(TWO, &[&group("on"), &group("off"), &format!("<< /Type /OCMD /OCGs {ocgs} /P {p} >>")]);
        match want {
            Some(w) => visible(&of(&doc, 7), w),
            None => {
                unresolved(&of(&doc, 7), &[(Rule::DeletedMember, true)]);
                visible(&verdict(&doc, &[(7, false)], Object::Ref(r(7))), false);
                visible(&verdict(&doc, &[(7, true)], Object::Ref(r(7))), true);
            }
        }
    }
    // With a visibility expression: the standard's reading has no group, so /VE alone decides;
    // the display's reading has one, and disagrees.
    let doc = file(TWO, &[&group("on"), &group("off"), "<< /Type /OCMD /VE [/Not 6 0 R] /OCGs [99 0 R] /P /AllOff >>"]);
    unresolved(&of(&doc, 7), &[(Rule::ExpressionDisagrees, true), (Rule::DeletedMember, true)]);
    let doc = file(TWO, &[&group("on"), &group("off"), "<< /Type /OCMD /VE [/Not 6 0 R] /OCGs [99 0 R] /P /AnyOn >>"]);
    visible(&of(&doc, 7), true);
    // Inline, nothing can name the dictionary, so no decision settles it.
    let doc = file(TWO, &[&group("on"), &group("off")]);
    let mut d = pdfcraft_cos::Dict::new();
    d.set(b"Type".to_vec(), Object::name("OCMD"));
    d.set(b"OCGs".to_vec(), Object::Array(vec![Object::Ref(r(99))]));
    d.set(b"P".to_vec(), Object::name("AllOff"));
    unresolved(&verdict(&doc, &[], Object::Dict(d)), &[(Rule::DeletedMember, false)]);
}

/// Members that aren't groups.
#[test]
fn a_malformed_membership_is_unresolved() {
    for ocgs in ["/OCGs [5 0 R 8 0 R]", "/OCGs [5 0 R 9]", "/OCGs (x)", "/OCGs << /Type /OCG /Name (direct) >>", "/OCGs [5 0 R 7 0 R]", "/OCGs 8 0 R"]
    {
        let doc = file(TWO, &[&group("on"), &group("off"), &format!("<< /Type /OCMD {ocgs} >>"), "<< /Type /Font >>"]);
        unresolved(&of(&doc, 7), &[(Rule::MalformedMembership, true)]);
    }
}

/// Three-valued logic: a result that doesn't depend on an unresolved member is resolved.
#[test]
fn results_that_dont_depend_on_an_unresolved_member_are_resolved() {
    // 5 on, 6 off, 7 Design (unresolved).
    let objs = |p: &str| [group("on"), group("off"), "<< /Type /OCG /Name (design) /Intent /Design >>".to_string(), format!("<< /Type /OCMD {p} >>")];
    let props = "/OCProperties << /OCGs [5 0 R 6 0 R 7 0 R] /D << /OFF [6 0 R] >> >>";
    let cases = [
        ("/OCGs [5 0 R 7 0 R] /P /AnyOn", Some(true)),
        ("/OCGs [6 0 R 7 0 R] /P /AllOn", Some(false)),
        ("/OCGs [6 0 R 7 0 R] /P /AnyOn", None),
        ("/OCGs [5 0 R 7 0 R] /P /AllOn", None),
        ("/OCGs [6 0 R 7 0 R] /P /AnyOff", Some(true)),
        ("/OCGs [5 0 R 7 0 R] /P /AllOff", Some(false)),
        ("/VE [/Or 5 0 R 7 0 R]", Some(true)),
        ("/VE [/And 6 0 R 7 0 R]", Some(false)),
        ("/VE [/Not 7 0 R]", None),
    ];
    for (p, want) in cases {
        let o = objs(p);
        let doc = file(props, &[&o[0], &o[1], &o[2], &o[3]]);
        match want {
            Some(w) => visible(&of(&doc, 8), w),
            None => unresolved(&of(&doc, 8), &[(Rule::IntentMismatch, true)]),
        }
    }
}

/// Rows 7 and 8: visibility expressions, nested three deep, and how they combine with `/P`.
#[test]
fn visibility_expressions() {
    // 5 on, 6 off.
    let cases = [
        ("[/And 5 0 R 6 0 R]", false),
        ("[/Or 5 0 R 6 0 R]", true),
        ("[/Not 6 0 R]", true),
        ("[/Not 5 0 R]", false),
        ("[/And 5 0 R [/Or 6 0 R [/Not 6 0 R]]]", true),
        ("[/Or 6 0 R [/And 5 0 R [/Not [/Or 6 0 R]]]]", true),
        ("[/And 5 0 R]", true),
    ];
    for (ve, want) in cases {
        let doc = file(TWO, &[&group("on"), &group("off"), &format!("<< /Type /OCMD /VE {ve} >>")]);
        visible(&of(&doc, 7), want);
        let mut ev = Evaluator::new(&doc, &Visibility::default());
        ev.visibility(&Object::Ref(r(7)));
        // No /OCGs: pdfcraft's display, which reads only /OCGs and /P, shows the content.
        assert_eq!(
            ev.notes(),
            [Note::ExpressionIgnoredByDisplay { membership: Some(r(7)), with_policy: false, display_differs: Some(!want) }],
            "{ve}"
        );
    }
    // An expression in an indirect array.
    let doc = file(TWO, &[&group("on"), &group("off"), "<< /Type /OCMD /VE 8 0 R >>", "[/Or 6 0 R 9 0 R]", "[/Not 6 0 R]"]);
    visible(&of(&doc, 7), true);
}

/// OV4.7, F1: with `/VE` and `/OCGs` + `/P` both present, both readings are evaluated, and they
/// must both resolve and agree. ISO 32000-1 prefers `/VE` only in an informative note
/// (§8.11.2.2, NOTE 2), and pdfcraft's display reads `/P`, so a disagreement is unresolved; an
/// explicit decision on the dictionary settles it.
#[test]
fn an_expression_and_a_policy_must_agree() {
    // 5 on, 6 off.
    for (ve, p, want) in [
        // Agree: visible, and hidden.
        ("[/Not 6 0 R]", "/OCGs [5 0 R] /P /AnyOn", Some(true)),
        ("[/And 5 0 R 6 0 R]", "/OCGs [5 0 R 6 0 R] /P /AllOn", Some(false)),
        ("[/Or 5 0 R]", "/OCGs 5 0 R", Some(true)),
        // Disagree, both ways: the review's case E4 (/VE shows, /P hides), and the reverse.
        ("[/Or 5 0 R]", "/OCGs [6 0 R] /P /AllOn", None),
        ("[/Not 5 0 R]", "/OCGs [5 0 R] /P /AnyOn", None),
        ("[/Not 5 0 R]", "/OCGs [5 0 R]", None),
    ] {
        let doc = file(TWO, &[&group("on"), &group("off"), &format!("<< /Type /OCMD /VE {ve} {p} >>")]);
        let mut ev = Evaluator::new(&doc, &Visibility::default());
        let v = ev.visibility(&Object::Ref(r(7)));
        match want {
            Some(w) => {
                visible(&v, w);
                assert_eq!(
                    ev.notes(),
                    [Note::ExpressionIgnoredByDisplay { membership: Some(r(7)), with_policy: true, display_differs: Some(false) }],
                    "{ve} {p}"
                );
            }
            None => {
                unresolved(&v, &[(Rule::ExpressionDisagrees, true)]);
                assert!(ev.notes().is_empty(), "{:?}", ev.notes());
                visible(&verdict(&doc, &[(7, true)], Object::Ref(r(7))), true);
                visible(&verdict(&doc, &[(7, false)], Object::Ref(r(7))), false);
            }
        }
    }
    // One reading unresolved: the dictionary is unresolved with that reading's problem.
    for (ve, p, rule) in [
        ("[/Not 5 0 R]", "/OCGs [5 0 R] /P /Bogus", Rule::UnknownPolicy),
        ("[/Not 6 0 R]", "/OCGs [5 0 R] /P /Bogus", Rule::UnknownPolicy),
        ("[/Not 5 0 R]", "/OCGs [5 0 R 7] /P /AnyOn", Rule::MalformedMembership),
        ("[/Xor 5 0 R]", "/OCGs [5 0 R] /P /AnyOn", Rule::MalformedExpression),
    ] {
        let doc = file(TWO, &[&group("on"), &group("off"), &format!("<< /Type /OCMD /VE {ve} {p} >>")]);
        unresolved(&of(&doc, 7), &[(rule, true)]);
        visible(&verdict(&doc, &[(7, true)], Object::Ref(r(7))), true);
    }
    // A member unresolved (a Design group under View): unresolved with the group's problem,
    // which a decision on the group settles, then compared again.
    let props = "/OCProperties << /OCGs [5 0 R 6 0 R 8 0 R] /D << /OFF [6 0 R] >> >>";
    let design = "<< /Type /OCG /Name (design) /Intent /Design >>";
    let doc = file(props, &[&group("on"), &group("off"), "<< /Type /OCMD /VE [/Or 5 0 R] /OCGs [8 0 R] /P /AllOn >>", design]);
    unresolved(&of(&doc, 7), &[(Rule::IntentMismatch, true)]);
    visible(&verdict(&doc, &[(8, true)], Object::Ref(r(7))), true);
    unresolved(&verdict(&doc, &[(8, false)], Object::Ref(r(7))), &[(Rule::ExpressionDisagrees, true)]);
}

/// OV4.7, F7: a default configuration whose `/Intent` isn't View (ISO 32000-1 Table 101 says it
/// shall be) is followed as written, with a note.
#[test]
fn a_configuration_intent_other_than_view_is_noted() {
    for (intent, note) in [
        ("", None),
        ("/Intent /View", None),
        ("/Intent [/View]", None),
        ("/Intent /All", Some(vec!["All"])),
        ("/Intent [/View /Design]", Some(vec!["View", "Design"])),
    ] {
        let doc = file(&format!("/OCProperties << /OCGs [5 0 R] /D << {intent} >> >>"), &[&group("x")]);
        let ev = Evaluator::new(&doc, &Visibility::default());
        let want: Vec<Note> = note.into_iter().map(|i| Note::ConfigurationIntent { intents: i.into_iter().map(String::from).collect() }).collect();
        assert_eq!(ev.notes(), want, "{intent}");
    }
}

/// Row 8b: malformed expressions, each unresolved with no fallback to `/P`.
#[test]
fn malformed_expressions() {
    let nest = |depth: usize| {
        let mut s = "5 0 R".to_string();
        for _ in 0..depth {
            s = format!("[/Not {s}]");
        }
        s
    };
    let cases = [
        "[/Xor 5 0 R 6 0 R]".to_string(),
        "[/Not 5 0 R 6 0 R]".to_string(),
        "[/Not]".to_string(),
        "[/And]".to_string(),
        "[]".to_string(),
        "[/And 5 0 R 3]".to_string(),
        "[/And 5 0 R 8 0 R]".to_string(),
        "[/And 5 0 R << /Type /OCG /Name (direct) >>]".to_string(),
        "9 0 R".to_string(),
        "(x)".to_string(),
        nest(34),
    ];
    for ve in cases {
        let doc = file(
            TWO,
            &[
                &group("on"),
                &group("off"),
                &format!("<< /Type /OCMD /VE {ve} /OCGs [5 0 R] >>"),
                "<< /Type /OCMD /OCGs [5 0 R] >>",
                "[/Or 6 0 R 9 0 R]",
            ],
        );
        unresolved(&of(&doc, 7), &[(Rule::MalformedExpression, true)]);
    }
    // 32 levels below the top expression is still allowed.
    let doc = file(TWO, &[&group("on"), &group("off"), &format!("<< /Type /OCMD /VE {} >>", nest(33))]);
    visible(&of(&doc, 7), false);
}

/// Row 13: references that aren't optional content, and inline dictionaries.
#[test]
fn references_that_arent_optional_content() {
    let doc = file(TWO, &[&group("on"), &group("off"), "<< /Type /Font >>", "<< /Name (no type) >>", "42"]);
    for n in [7, 8, 9, 99] {
        unresolved(&of(&doc, n), &[(Rule::NotOptionalContent, true)]);
    }
    visible(&verdict(&doc, &[(8, true)], Object::Ref(r(8))), true);
    let inline = |s: &str| match pdfcraft_content::parse(format!("{s} pop").as_bytes()).ops.first().and_then(|o| o.operands.first().cloned()) {
        Some(o) => o,
        None => panic!("parses"),
    };
    unresolved(&verdict(&doc, &[], inline("<< /Type /OCG /Name (x) >>")), &[(Rule::Inline, false)]);
    unresolved(&verdict(&doc, &[], inline("<< /Foo 1 >>")), &[(Rule::Inline, false)]);
    unresolved(&verdict(&doc, &[], Object::Int(3)), &[(Rule::NotOptionalContent, false)]);
    // A direct membership dictionary is evaluated; its own problems can't be settled by name.
    visible(&verdict(&doc, &[], inline("<< /Type /OCMD /OCGs [5 0 R] >>")), true);
    unresolved(&verdict(&doc, &[], inline("<< /Type /OCMD /OCGs [5 0 R] /P /Bogus >>")), &[(Rule::UnknownPolicy, false)]);
    visible(&verdict(&doc, &[], inline("<< /Type /OCMD /OCGs [] >>")), true);
}

// ── Auto-states ─────────────────────────────────────────────────────────────────────────────

fn with_usage(usage: &str) -> String {
    format!("<< /Type /OCG /Name (u) /Usage << {usage} >> >>")
}

fn auto(entries: &str) -> String {
    format!("/OCProperties << /OCGs [5 0 R 6 0 R] /D << /OFF [6 0 R] /AS [{entries}] >> >>")
}

/// View auto-states with category View: unresolved when they would change the group, recorded
/// when they wouldn't (until the clause is confirmed).
#[test]
fn view_auto_states() {
    let entry = "<< /Event /View /Category [/View] /OCGs [5 0 R 6 0 R] >>";
    let doc = file(&auto(entry), &[&with_usage("/View << /ViewState /OFF >>"), &with_usage("/View << /ViewState /OFF >>")]);
    unresolved(&of(&doc, 5), &[(Rule::AutoStateView, true)]);
    visible(&of(&doc, 6), false);
    let mut ev = Evaluator::new(&doc, &Visibility::default());
    ev.visibility(&Object::Ref(r(6)));
    assert!(ev.notes().contains(&Note::AutoStateNoChange { group: r(6) }));
    visible(&verdict(&doc, &[(5, false)], Object::Ref(r(5))), false);
    // A group without a View usage entry isn't affected.
    let doc = file(&auto(entry), &[&group("plain"), &with_usage("/Print << /PrintState /ON >>")]);
    visible(&of(&doc, 5), true);
    visible(&of(&doc, 6), false);
    // A malformed view state.
    let doc = file(&auto(entry), &[&with_usage("/View << /ViewState /Maybe >>"), &group("x")]);
    unresolved(&of(&doc, 5), &[(Rule::MalformedAutoState, true)]);
}

/// View auto-states with categories that depend on the viewer.
#[test]
fn environment_auto_states() {
    for (cat, usage) in [
        ("/Zoom", "/Zoom << /min 2 >>"),
        ("/Language", "/Language << /Lang (fr) /Preferred /ON >>"),
        ("/User", "/User << /Type /Ind /Name (x) >>"),
        ("/View /Zoom", "/Zoom << /max 1 >>"),
    ] {
        let doc = file(&auto(&format!("<< /Event /View /Category [{cat}] /OCGs [5 0 R] >>")), &[&with_usage(usage), &group("x")]);
        unresolved(&of(&doc, 5), &[(Rule::AutoStateEnvironment, true)]);
        visible(&verdict(&doc, &[(5, true)], Object::Ref(r(5))), true);
        // The same category on a group without that usage entry changes nothing.
        let doc = file(&auto(&format!("<< /Event /View /Category [{cat}] /OCGs [5 0 R] >>")), &[&group("plain"), &group("x")]);
        visible(&of(&doc, 5), true);
    }
}

/// Print and Export auto-states aren't applied, and the report says so.
#[test]
fn print_and_export_auto_states_are_not_applied() {
    for event in ["/Print", "/Export"] {
        let doc = file(
            &auto(&format!("<< /Event {event} /Category [/Print /Export] /OCGs [5 0 R 6 0 R] >>")),
            &[&with_usage("/Print << /PrintState /OFF >> /Export << /ExportState /OFF >>"), &group("x")],
        );
        let mut ev = Evaluator::new(&doc, &Visibility::default());
        visible(&ev.visibility(&Object::Ref(r(5))), true);
        assert_eq!(ev.notes(), [Note::PrintExportNotApplied { groups: vec![r(5), r(6)] }]);
    }
}

/// Malformed auto-states.
#[test]
fn malformed_auto_states() {
    for entry in [
        "<< /Event /Hover /Category [/View] /OCGs [5 0 R] >>",
        "<< /Category [/View] /OCGs [5 0 R] >>",
        "<< /Event /View /OCGs [5 0 R] >>",
        "<< /Event /View /Category /View /OCGs [5 0 R] >>",
    ] {
        let doc = file(&auto(entry), &[&with_usage("/View << /ViewState /ON >>"), &group("x")]);
        unresolved(&of(&doc, 5), &[(Rule::MalformedAutoState, true)]);
        visible(&of(&doc, 6), false);
    }
    for props in [
        "/OCProperties << /OCGs [5 0 R 6 0 R] /D << /AS 5 >> >>",
        "/OCProperties << /OCGs [5 0 R 6 0 R] /D << /AS [3] >> >>",
        &auto("<< /Event /View /Category [/View] /OCGs 7 >>"),
    ] {
        let doc = file(props, &[&group("a"), &group("b")]);
        unresolved(&of(&doc, 5), &[(Rule::MalformedAutoState, false)]);
    }
}

// ── Decisions ───────────────────────────────────────────────────────────────────────────────

/// Row 14: an explicit decision fixes the visibility of the object it names, and nothing else.
#[test]
fn explicit_decisions_settle_exactly_what_they_name() {
    let doc = file(TWO, &[&group("on"), &group("off"), "<< /Type /OCMD /OCGs [5 0 R 6 0 R] /P /AllOn >>"]);
    visible(&verdict(&doc, &[(5, false)], Object::Ref(r(5))), false);
    visible(&verdict(&doc, &[(5, false)], Object::Ref(r(6))), false);
    visible(&verdict(&doc, &[(6, true)], Object::Ref(r(7))), true);
    visible(&verdict(&doc, &[(7, false), (6, true)], Object::Ref(r(7))), false);
    visible(&verdict(&doc, &[(7, true)], Object::Ref(r(5))), true);
}

// ── No panics ───────────────────────────────────────────────────────────────────────────────

/// L14 for the evaluator: 10,000 seeded hostile configurations and references, each evaluated
/// in bounded time, never a panic.
#[test]
fn hostile_configurations_never_panic() {
    let mut rng = Rng(0x0154_0002);
    let atoms = [
        "5 0 R",
        "6 0 R",
        "7 0 R",
        "8 0 R",
        "9 0 R",
        "99 0 R",
        "null",
        "1",
        "(x)",
        "/View",
        "/Design",
        "/All",
        "/ON",
        "/OFF",
        "/Unchanged",
        "[]",
        "<< >>",
        "/And",
        "/Or",
        "/Not",
        "/AnyOn",
        "/AllOff",
        "/Bogus",
    ];
    let value = |rng: &mut Rng, depth: usize| -> String {
        fn go(rng: &mut Rng, depth: usize, atoms: &[&str]) -> String {
            match rng.next_u64() % 6 {
                0 if depth < 40 => {
                    let n = rng.next_u64() % 4;
                    let items: Vec<String> = (0..n).map(|_| go(rng, depth + 1, atoms)).collect();
                    format!("[{}]", items.join(" "))
                }
                1 if depth < 40 => format!("[/{} {}]", ["And", "Or", "Not"][(rng.next_u64() % 3) as usize], go(rng, depth + 1, atoms)),
                _ => atoms[(rng.next_u64() % atoms.len() as u64) as usize].to_string(),
            }
        }
        go(rng, depth, &atoms)
    };
    let start = Instant::now();
    let (mut resolved, mut unresolved_count) = (0, 0);
    for _ in 0..10_000 {
        let props = format!(
            "/OCProperties << /OCGs {} /D << /BaseState {} /ON {} /OFF {} /Intent {} /AS [<< /Event {} /Category {} /OCGs {} >>] >> >>",
            value(&mut rng, 0),
            value(&mut rng, 0),
            value(&mut rng, 0),
            value(&mut rng, 0),
            value(&mut rng, 0),
            value(&mut rng, 0),
            value(&mut rng, 0),
            value(&mut rng, 0)
        );
        let objs = [
            format!(
                "<< /Type /OCG /Name (a) /Intent {} /Usage << /View << /ViewState {} >> /Zoom << >> >> >>",
                value(&mut rng, 0),
                value(&mut rng, 0)
            ),
            format!("<< /Type /OCMD /OCGs {} /P {} /VE {} >>", value(&mut rng, 0), value(&mut rng, 0), value(&mut rng, 0)),
            format!("<< /Type /OCMD /VE {} >>", value(&mut rng, 0)),
            value(&mut rng, 0),
            format!("[/And 9 0 R {}]", value(&mut rng, 0)),
        ];
        let doc = file(&props, &objs.iter().map(String::as_str).collect::<Vec<_>>());
        let decisions: Vec<(ObjRef, bool)> =
            (0..rng.next_u64() % 3).map(|_| (r(5 + (rng.next_u64() % 5) as u32), rng.next_u64().is_multiple_of(2))).collect();
        let mut ev = Evaluator::new(&doc, &Visibility { decisions, ..Visibility::default() });
        for n in 5..=10 {
            let v = ev.visibility(&Object::Ref(r(n)));
            assert!(v.visible.is_some() == v.problems.is_empty(), "{v:?}");
            if v.visible.is_some() {
                resolved += 1;
            } else {
                unresolved_count += 1;
            }
        }
    }
    println!("hostile sweep: {resolved} resolved, {unresolved_count} unresolved, {:?}", start.elapsed());
    assert!(resolved > 1_000 && unresolved_count > 1_000, "both outcomes are exercised: {resolved} resolved, {unresolved_count} unresolved");
    assert!(start.elapsed().as_secs() < 120, "10,000 cases took {:?}", start.elapsed());
}
