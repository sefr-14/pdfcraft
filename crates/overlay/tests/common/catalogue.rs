//! The T4 and T5 layered cases with their twins (step 4 PRD, L5, L6): one catalogue, shared by
//! the resolver's tests and the step 4 proof, so the proof renders exactly the cases the tests
//! check. Each case says what shows (the `shown` flags of its layers): the generator's own
//! knowledge, never the resolver's.

#![allow(dead_code)]

use super::twins::Node::{Begin, Canary, Do, End, Layer, Live, LiveClip, Ops, Tag};
use super::twins::{Case, Node, XObject};

pub fn group(name: &str) -> String {
    format!("<< /Type /OCG /Name ({name}) >>")
}

pub fn groups3() -> Vec<(&'static str, String)> {
    vec![("G1", group("Base")), ("G2", group("Notes")), ("G3", group("Hatch"))]
}

pub fn props3() -> Vec<(&'static str, &'static str)> {
    vec![("L1", "G1"), ("L2", "G2"), ("L3", "G3")]
}

/// A visible drawing in an ordinary colour, so the overlay has something to show.
pub const BASE: Node = Ops("0.2 g 20 20 100 40 re f");
pub const MORE: Node = Ops("0 0 1 RG 3 w 30 200 m 250 200 l S");

/// The default configuration most cases use: G2 off.
pub const D: &str = "<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>";

fn intent_groups() -> Vec<(&'static str, String)> {
    vec![
        ("G1", "<< /Type /OCG /Name (Both) /Intent [/View /Design] >>".to_string()),
        ("G2", group("Plain")),
        ("G3", "<< /Type /OCG /Name (Design) /Intent /Design >>".to_string()),
    ]
}

/// A case with the three groups (and an `M` membership dictionary when `objects` has one) under
/// configuration `props`.
pub fn simple(name: &'static str, objects: Vec<(&'static str, String)>, props: &str, content: Vec<Node>) -> Case {
    let properties =
        [("L1", "G1"), ("L2", "G2"), ("L3", "G3"), ("M", "M")].into_iter().filter(|(_, o)| objects.iter().any(|(n, _)| n == o)).collect();
    Case { name, objects, oc_properties: Some(props.to_string()), properties, contents: vec![content], ..Case::default() }
}

pub fn with_ocmd(body: &str) -> Vec<(&'static str, String)> {
    let mut o = groups3();
    o.push(("M", body.to_string()));
    o
}

/// T5: G1, G2 and M (AllOn of G1 and G2) under `decisions`; the flags say what shows.
pub fn t5(name: &'static str, decisions: Vec<(&'static str, bool)>, g1: bool, g2: bool, m: bool) -> Case {
    let mut objects = groups3();
    objects.push(("M", "<< /Type /OCMD /OCGs [{G1} {G2}] /P /AllOn >>".into()));
    let node = |shown: bool, colour: &'static str| if shown { Ops(colour) } else { Canary };
    Case {
        name,
        objects,
        oc_properties: Some(D.into()),
        properties: vec![("L1", "G1"), ("L2", "G2"), ("M", "M")],
        contents: vec![vec![
            BASE,
            Layer("L1", g1, vec![node(g1, "0.1 g 10 150 30 30 re f")]),
            Layer("L2", g2, vec![node(g2, "0.4 g 60 150 30 30 re f")]),
            Layer("M", m, vec![node(m, "0.7 g 110 150 30 30 re f")]),
        ]],
        decisions,
        ..Case::default()
    }
}

/// Two groups the configuration doesn't list, decided as given.
pub fn settles(decisions: Vec<(&'static str, bool)>) -> Case {
    let mut objects = groups3();
    objects.push(("U1", group("Loose 1")));
    objects.push(("U2", group("Loose 2")));
    Case {
        name: "settles",
        objects,
        oc_properties: Some(D.into()),
        properties: vec![("U1", "U1"), ("U2", "U2")],
        contents: vec![vec![BASE, Layer("U1", false, vec![Canary]), Layer("U2", false, vec![Canary])]],
        decisions,
        ..Case::default()
    }
}

fn case_as_view_no_change_and_print() -> Case {
    let mut g = groups3();
    g[1].1 = "<< /Type /OCG /Name (Notes) /Usage << /View << /ViewState /ON >> /Print << /PrintState /ON >> >> >>".into();
    simple(
        "as-view-no-change-and-print",
        g,
        "<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G3}] /AS [<< /Event /View /Category [/View] /OCGs [{G2}] >> << /Event /Print /Category [/Print] /OCGs [{G2}] >>] >> >>",
        vec![BASE, Layer("L2", true, vec![MORE]), Layer("L3", false, vec![Canary])],
    )
}

fn case_base_on_off_list() -> Case {
    Case {
        name: "base-on-off-list",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![BASE, Layer("L1", true, vec![MORE]), Layer("L2", false, vec![Canary]), Ops("0.5 g 150 20 40 40 re f")]],
        ..Case::default()
    }
}

fn case_base_off_on_list() -> Case {
    Case {
        name: "base-off-on-list",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /BaseState /OFF /ON [{G1}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![BASE, Layer("L1", true, vec![MORE]), Layer("L2", false, vec![Canary]), Layer("L3", false, vec![Canary])]],
        ..Case::default()
    }
}

fn case_on_off_lists() -> Case {
    Case {
        name: "on-off-lists",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /BaseState /ON /ON [{G1}] /OFF [{G2} {G3}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![Layer("L3", false, vec![Canary]), BASE, Layer("L1", true, vec![MORE]), Layer("L2", false, vec![Canary])]],
        ..Case::default()
    }
}

fn case_nested_3_deep() -> Case {
    Case {
        name: "nested-3-deep",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G3}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![
            BASE,
            Layer(
                "L1",
                true,
                vec![
                    Ops("1 0 0 rg 10 150 30 30 re f"),
                    Layer("L2", true, vec![MORE, Layer("L3", false, vec![Canary]), Ops("0 1 0 rg 60 150 30 30 re f")]),
                ],
            ),
            // Visible layers inside a hidden one stay hidden.
            Layer("L3", false, vec![Layer("L1", true, vec![Canary, Layer("L2", true, vec![Canary])])]),
        ]],
        ..Case::default()
    }
}

fn case_ocmd_policies() -> Case {
    let mut objects = groups3();
    for (name, body) in [
        ("M1", "<< /Type /OCMD /OCGs [{G1} {G2}] /P /AnyOn >>"),
        ("M2", "<< /Type /OCMD /OCGs [{G1} {G2}] /P /AllOn >>"),
        ("M3", "<< /Type /OCMD /OCGs [{G1} {G2}] /P /AnyOff >>"),
        ("M4", "<< /Type /OCMD /OCGs [{G1} {G2}] /P /AllOff >>"),
        ("M5", "<< /Type /OCMD /OCGs {G1} /P /AllOn >>"),
        ("M6", "<< /Type /OCMD /OCGs {G2} /P /AllOff >>"),
        ("M7", "<< /Type /OCMD /OCGs [{G1} {G2}] >>"),
    ] {
        objects.push((name, body.to_string()));
    }
    Case {
        name: "ocmd-policies",
        objects,
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: vec![("A1", "M1"), ("A2", "M2"), ("A3", "M3"), ("A4", "M4"), ("A5", "M5"), ("A6", "M6"), ("A7", "M7")],
        contents: vec![vec![
            BASE,
            Layer("A1", true, vec![Ops("0.1 g 10 150 20 20 re f")]),
            Layer("A2", false, vec![Canary]),
            Layer("A3", true, vec![Ops("0.3 g 40 150 20 20 re f")]),
            Layer("A4", false, vec![Canary]),
            Layer("A5", true, vec![Ops("0.5 g 70 150 20 20 re f")]),
            Layer("A6", true, vec![Ops("0.6 g 100 150 20 20 re f")]),
            Layer("A7", true, vec![Ops("0.7 g 130 150 20 20 re f")]),
        ]],
        ..Case::default()
    }
}

fn case_visibility_expressions() -> Case {
    let mut objects = groups3();
    for (name, body) in [
        ("E1", "<< /Type /OCMD /VE [/And {G1} [/Not {G2}]] >>"),
        ("E2", "<< /Type /OCMD /VE [/Or {G2} [/And {G1} {G2}]] >>"),
        // Three deep: Not(Or(And(G1, G2), Not(G1))) = Not(Or(false, false)) = true.
        ("E3", "<< /Type /OCMD /VE [/Not [/Or [/And {G1} {G2}] [/Not {G1}]]] >>"),
        // /VE and /P both present: they must agree (OV4.7, F1). Both show, and both hide.
        ("E4", "<< /Type /OCMD /OCGs [{G1}] /P /AllOn /VE [/Or {G1}] >>"),
        ("E5", "<< /Type /OCMD /OCGs [{G2}] /P /AnyOn /VE [/And {G1} {G2}] >>"),
    ] {
        objects.push((name, body.to_string()));
    }
    Case {
        name: "visibility-expressions",
        objects,
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: vec![("X1", "E1"), ("X2", "E2"), ("X3", "E3"), ("X4", "E4"), ("X5", "E5")],
        contents: vec![vec![
            BASE,
            Layer("X1", true, vec![Ops("0.1 g 10 150 20 20 re f")]),
            Layer("X2", false, vec![Canary]),
            Layer("X3", true, vec![Ops("0.3 g 40 150 20 20 re f")]),
            Layer("X4", true, vec![Ops("0.4 g 70 150 20 20 re f")]),
            Layer("X5", false, vec![Canary]),
        ]],
        ..Case::default()
    }
}

fn case_oc_on_form_and_image() -> Case {
    Case {
        name: "oc-on-form-and-image",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![BASE, Do("F1"), Do("F2"), Ops("q 40 0 0 40 150 150 cm"), Do("I1"), Ops("Q q 40 0 0 40 200 150 cm"), Do("I2"), Ops("Q")]],
        xobjects: vec![
            XObject::form("F1", vec![Ops("0 0.5 0 rg 10 100 30 30 re f")]).with_oc("G1", true),
            XObject::form("F2", vec![Canary]).with_oc("G2", false),
            XObject::image("I1").with_oc("G1", true),
            XObject::image("I2").with_oc("G2", false),
        ],
        ..Case::default()
    }
}

fn case_form_in_visible_and_hidden() -> Case {
    Case {
        name: "form-in-visible-and-hidden",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![
            BASE,
            Layer("L1", true, vec![Do("F3")]),
            Layer("L2", false, vec![Ops("q 1 0 0 1 50 50 cm"), Do("F3"), Ops("Q"), Canary]),
        ]],
        xobjects: vec![XObject::form("F3", vec![Ops("0 0 0.5 rg 100 100 40 40 re f")])],
        ..Case::default()
    }
}

fn case_split_across_streams() -> Case {
    Case {
        name: "split-across-streams",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![BASE, Begin("L2", false)], vec![Canary, End, Begin("L1", true), MORE], vec![Ops("0.4 g 200 20 30 30 re f"), End]],
        ..Case::default()
    }
}

fn case_tagged_text() -> Case {
    Case {
        name: "tagged-text",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![
            BASE,
            Layer("L1", true, vec![Tag("/P << /MCID 0 >> BDC", vec![Ops("BT /FC 12 Tf 20 260 Td (Visible) Tj ET")])]),
            Layer("L2", false, vec![Tag("/P << /MCID 1 >> BDC", vec![Canary])]),
            Tag("/Span << /ActualText (x) >> BDC", vec![Layer("L1", true, vec![MORE])]),
        ]],
        ..Case::default()
    }
}

fn case_intent_all() -> Case {
    let objects = intent_groups();
    Case {
        name: "intent-all",
        objects,
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /Intent /All /OFF [{G3}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![
            BASE,
            Layer("L1", true, vec![MORE]),
            Layer("L2", true, vec![Ops("0.6 g 200 20 30 30 re f")]),
            Layer("L3", false, vec![Canary]),
        ]],
        ..Case::default()
    }
}

fn case_intent_view_design() -> Case {
    let objects = intent_groups();
    Case {
        name: "intent-view-design",
        objects,
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /Intent [/View /Design] /OFF [{G2}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![
            BASE,
            Layer("L1", true, vec![MORE]),
            Layer("L2", false, vec![Canary]),
            Layer("L3", true, vec![Ops("0.6 g 200 20 30 30 re f")]),
        ]],
        ..Case::default()
    }
}

fn case_inside_forms() -> Case {
    let mask = XObject { group: true, ..XObject::form("SMG", vec![Ops("1 g 0 0 300 300 re f"), Layer("L2", false, vec![Canary])]) };
    Case {
        name: "inside-forms",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![BASE, Do("F4"), Ops("q /Pattern cs /P1 scn 150 150 100 100 re f Q"), Ops("q /SM gs 0 0 0.6 rg 150 20 60 60 re f Q")]],
        xobjects: vec![
            XObject::form("F4", vec![Layer("L1", true, vec![Ops("0.3 0 0 rg 10 100 40 40 re f")]), Layer("L2", false, vec![Canary])]),
            mask,
        ],
        patterns: vec![("P1", vec![Ops("0 0.4 0 rg 0 0 10 10 re f"), Layer("L2", false, vec![Canary])])],
        soft_masks: vec![("SM", "SMG")],
        ..Case::default()
    }
}

fn case_live_state() -> Case {
    Case {
        name: "live-state",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![
            BASE,
            Layer("L2", false, vec![Ops("0.9 g"), Live("0.2 0.4 0.6 rg"), Canary, Live("1 0 0 1 10 0 cm"), Live("4 w")]),
            Ops("150 20 40 40 re f"),
            // Overwritten before use: the hidden block's colour isn't kept.
            Layer("L2", false, vec![Ops("0.8 0.1 0.1 rg"), Canary]),
            Ops("0 0 0 rg 0 0 1 RG 20 150 m 120 150 l S"),
            // A clipping path later content is drawn through.
            Layer("L2", false, vec![LiveClip("100 100 100 100"), Canary]),
            Ops("0.5 g 0 0 300 300 re f"),
        ]],
        ..Case::default()
    }
}

/// OV4.7, F1: `/VE` and `/P` that disagree (the review's E4), settled by a decision on the
/// dictionary.
pub fn ve_p_disagree(name: &'static str, decided: Option<bool>) -> Case {
    let shown = decided.unwrap_or(false);
    Case {
        decisions: decided.map(|d| vec![("M", d)]).unwrap_or_default(),
        ..simple(
            name,
            with_ocmd("<< /Type /OCMD /OCGs [{G2}] /P /AllOn /VE [/Or {G1}] >>"),
            D,
            vec![BASE, Layer("M", shown, vec![if shown { Ops("0.4 g 70 150 20 20 re f") } else { Canary }])],
        )
    }
}

/// Every resolved T4 and T5 case, in a fixed order.
pub fn resolved() -> Vec<Case> {
    vec![
        case_base_on_off_list(),
        case_base_off_on_list(),
        case_on_off_lists(),
        case_nested_3_deep(),
        case_ocmd_policies(),
        case_visibility_expressions(),
        case_oc_on_form_and_image(),
        case_form_in_visible_and_hidden(),
        case_split_across_streams(),
        case_tagged_text(),
        case_intent_all(),
        case_intent_view_design(),
        case_inside_forms(),
        case_live_state(),
        case_as_view_no_change_and_print(),
        t5("default", vec![], true, false, false),
        t5("g2-on", vec![("G2", true)], true, true, true),
        t5("g1-off", vec![("G1", false)], false, false, false),
        t5("m-on", vec![("M", true)], true, false, true),
        settles(vec![("U1", false), ("U2", false)]),
        ve_p_disagree("ve-p-disagree-shown", Some(true)),
        ve_p_disagree("ve-p-disagree-hidden", Some(false)),
    ]
}

/// The resolved case called `name`.
pub fn get(name: &str) -> Case {
    resolved().into_iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no case {name}"))
}
