//! This fork (BOOKMARK.DND.1): `bookmark_move_group`, end to end through the tools: several
//! bookmarks to one place as one undoable step, saved and reopened, refused whole, and a dry run
//! that changes nothing. `bookmark_move` keeps its own contract.

use std::path::{Path, PathBuf};

use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

/// Three blank pages.
fn three_pages() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 200 300] >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Type /Page /Parent 2 0 R >>",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn workdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-bookmark-move-group-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.pdf"), three_pages()).unwrap();
    dir
}

fn auto(dir: &Path) -> Automation {
    Automation::new().with_root(dir).unwrap().with_clock(|| 1_700_000_000)
}

fn ok(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args) {
        Ok(mut c) => match c.remove(0) {
            Content::Json(v) => v,
            other => panic!("{tool}: expected JSON, got {other:?}"),
        },
        Err(e) => panic!("{tool} {args}: {e}"),
    }
}

/// The outline as `Title[Child,…]`, top level joined by commas.
fn outline(a: &mut Automation, doc: u64) -> String {
    fn level(list: &Value) -> String {
        list.as_array()
            .into_iter()
            .flatten()
            .map(|b| {
                let title = b["title"].as_str().unwrap_or_default();
                match b["children"].as_array() {
                    Some(c) if !c.is_empty() => format!("{title}[{}]", level(&b["children"])),
                    _ => title.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    }
    level(&ok(a, "bookmark_list", json!({ "doc": doc }))["bookmarks"])
}

/// A, B[b1,b2], C, D.
fn opened(a: &mut Automation) -> u64 {
    let doc = ok(a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    for (title, page) in [("A", 1), ("B", 2), ("C", 3), ("D", 1)] {
        ok(a, "bookmark_add", json!({ "doc": doc, "title": title, "page": page }));
    }
    for title in ["b1", "b2"] {
        ok(a, "bookmark_add", json!({ "doc": doc, "title": title, "page": 2, "parent": [2] }));
    }
    assert_eq!(outline(a, doc), "A,B[b1,b2],C,D");
    doc
}

#[test]
fn bookmarks_move_together_undo_and_save() {
    let dir = workdir("together");
    let mut a = auto(&dir);
    let doc = opened(&mut a);
    // D and b1, listed out of order, to before A: they arrive in outline order.
    let r = ok(&mut a, "bookmark_move_group", json!({ "doc": doc, "paths": [[4], [2, 1]], "position": 1 }));
    assert_eq!(r["paths"], json!([[1], [2]]));
    assert_eq!((r["changed"].as_bool(), r["dry_run"].as_bool(), r["undo"].as_str()), (Some(true), Some(false), Some("Move bookmarks")));
    assert_eq!(outline(&mut a, doc), "b1,D,A,B[b2],C");
    ok(&mut a, "edit_undo", json!({ "doc": doc }));
    assert_eq!(outline(&mut a, doc), "A,B[b1,b2],C,D", "one step");
    ok(&mut a, "edit_redo", json!({ "doc": doc }));
    // Under C, with no position: last. B takes b2 along.
    let r = ok(&mut a, "bookmark_move_group", json!({ "doc": doc, "paths": [[4], [1]], "parent": [5] }));
    assert_eq!(r["paths"], json!([[3, 1], [3, 2]]));
    assert_eq!(outline(&mut a, doc), "D,A,C[b1,B[b2]]");
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "moved.pdf" }));
    let mut b = auto(&dir);
    let re = ok(&mut b, "doc_open", json!({ "path": "moved.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(outline(&mut b, re), "D,A,C[b1,B[b2]]");
    // Destinations came along: B still goes to page 2.
    let listed = ok(&mut b, "bookmark_list", json!({ "doc": re }));
    assert_eq!(listed["bookmarks"][2]["children"][1]["page"], json!(2));
}

#[test]
fn a_dry_run_reports_the_move_and_changes_nothing() {
    let dir = workdir("dry");
    let mut a = auto(&dir);
    let doc = opened(&mut a);
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "saved.pdf" }));
    let before = ok(&mut a, "doc_list", json!({}));
    let args = json!({ "doc": doc, "paths": [[2], [4]], "parent": [1], "position": 1 });
    let mut dry_args = args.clone();
    dry_args["dry_run"] = json!(true);
    let dry = ok(&mut a, "bookmark_move_group", dry_args);
    assert_eq!(dry["paths"], json!([[1, 1], [1, 2]]));
    assert_eq!((dry["changed"].as_bool(), dry["dry_run"].as_bool()), (Some(true), Some(true)));
    assert_eq!(outline(&mut a, doc), "A,B[b1,b2],C,D", "nothing moved");
    assert_eq!(ok(&mut a, "doc_list", json!({})), before, "the same unsaved state and undo history");
    assert_eq!(dry["dirty"], json!(false), "a saved document is not marked edited by a dry run");
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "after-dry.pdf" }));
    assert_eq!(std::fs::read(dir.join("saved.pdf")).unwrap(), std::fs::read(dir.join("after-dry.pdf")).unwrap(), "the file is the same");
    // The real call then does what the dry run said.
    let real = ok(&mut a, "bookmark_move_group", args);
    assert_eq!(real["paths"], dry["paths"]);
    assert_eq!(outline(&mut a, doc), "A[B[b1,b2],D],C");
    // A dry run of a move that can't be made is refused, as the move would be.
    assert!(matches!(
        a.call("bookmark_move_group", &json!({ "doc": doc, "paths": [[1]], "parent": [1, 1], "dry_run": true })),
        Err(ToolError::Failed(_))
    ));
}

#[test]
fn a_move_to_the_same_place_reports_no_change_and_adds_no_undo_step() {
    let dir = workdir("same");
    let mut a = auto(&dir);
    let doc = opened(&mut a);
    let before = ok(&mut a, "doc_list", json!({}));
    // B is second: before C (position 3) is where it is.
    let r = ok(&mut a, "bookmark_move_group", json!({ "doc": doc, "paths": [[2]], "position": 3 }));
    assert_eq!((r["changed"].as_bool(), &r["paths"]), (Some(false), &json!([[2]])));
    assert_eq!(ok(&mut a, "doc_list", json!({})), before);
    assert_eq!(r["undo"].as_str(), Some("Add bookmark"), "the last undo step is still the one before");
}

#[test]
fn a_group_that_cannot_move_is_refused_whole() {
    let dir = workdir("refused");
    let mut a = auto(&dir);
    let doc = opened(&mut a);
    let failed = |a: &mut Automation, args: Value| match a.call("bookmark_move_group", &args) {
        Err(ToolError::Failed(m)) => m,
        other => panic!("{args}: expected a refusal, got {other:?}"),
    };
    // Into itself, into something under a bookmark of the group, a missing bookmark, a missing parent.
    assert!(failed(&mut a, json!({ "doc": doc, "paths": [[2]], "parent": [2] })).contains("inside itself"));
    assert!(failed(&mut a, json!({ "doc": doc, "paths": [[1], [2]], "parent": [2, 1] })).contains("inside itself"));
    assert!(failed(&mut a, json!({ "doc": doc, "paths": [[1], [9]] })).contains("no bookmark"));
    assert!(failed(&mut a, json!({ "doc": doc, "paths": [[1]], "parent": [7] })).contains("no bookmark"));
    assert_eq!(outline(&mut a, doc), "A,B[b1,b2],C,D", "nothing moved");
    for bad in [json!([]), json!([[]]), json!([[0]]), json!([1, 2]), json!("1")] {
        assert!(matches!(a.call("bookmark_move_group", &json!({ "doc": doc, "paths": bad })), Err(ToolError::InvalidArgs(_))), "{bad}");
    }
    for (key, bad) in [("position", json!(0)), ("position", json!("first")), ("parent", json!([0])), ("dry_run", json!("yes"))] {
        let mut args = json!({ "doc": doc, "paths": [[1]] });
        args[key] = bad.clone();
        assert!(matches!(a.call("bookmark_move_group", &args), Err(ToolError::InvalidArgs(_))), "{key} {bad}");
    }
    assert_eq!(outline(&mut a, doc), "A,B[b1,b2],C,D");
}

/// The single-bookmark tool is as it was: its position counts after the bookmark is taken out.
#[test]
fn bookmark_move_keeps_its_own_position_rule() {
    let dir = workdir("single");
    let mut a = auto(&dir);
    let doc = opened(&mut a);
    // A to position 2 after removal: after B.
    ok(&mut a, "bookmark_move", json!({ "doc": doc, "path": [1], "position": 2 }));
    assert_eq!(outline(&mut a, doc), "B[b1,b2],A,C,D");
    ok(&mut a, "edit_undo", json!({ "doc": doc }));
    // The group tool's position 2 is "before B as things stand": A stays first.
    let r = ok(&mut a, "bookmark_move_group", json!({ "doc": doc, "paths": [[1]], "position": 2 }));
    assert_eq!(r["changed"], json!(false));
    assert_eq!(outline(&mut a, doc), "A,B[b1,b2],C,D");
}
