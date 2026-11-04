//! This fork (BOOKMARK.1): `bookmark_indent` and `bookmark_outdent`, end to end through the tools:
//! one undoable step, saved and reopened, refused whole.

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
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-bookmark-shift-{}-{name}", std::process::id()));
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

#[test]
fn bookmarks_indent_and_outdent_together_undo_and_save() {
    let dir = workdir("together");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    for (title, page) in [("A", 1), ("B", 2), ("C", 3), ("D", 1)] {
        ok(&mut a, "bookmark_add", json!({ "doc": doc, "title": title, "page": page }));
    }
    ok(&mut a, "bookmark_indent", json!({ "doc": doc, "paths": [[2], [3]] }));
    assert_eq!(outline(&mut a, doc), "A[B,C],D");
    ok(&mut a, "edit_undo", json!({ "doc": doc }));
    assert_eq!(outline(&mut a, doc), "A,B,C,D", "one step");
    ok(&mut a, "edit_redo", json!({ "doc": doc }));
    ok(&mut a, "bookmark_outdent", json!({ "doc": doc, "paths": [[1, 1]] }));
    assert_eq!(outline(&mut a, doc), "A[C],B,D");
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "shifted.pdf" }));
    let mut b = auto(&dir);
    let re = ok(&mut b, "doc_open", json!({ "path": "shifted.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(outline(&mut b, re), "A[C],B,D");
}

#[test]
fn a_group_that_cannot_move_is_refused_whole() {
    let dir = workdir("refused");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    for title in ["A", "B", "C"] {
        ok(&mut a, "bookmark_add", json!({ "doc": doc, "title": title, "page": 1 }));
    }
    // B could go under A, but A is first at its level.
    assert!(matches!(a.call("bookmark_indent", &json!({ "doc": doc, "paths": [[2], [1]] })), Err(ToolError::Failed(_))));
    assert!(matches!(a.call("bookmark_outdent", &json!({ "doc": doc, "paths": [[3]] })), Err(ToolError::Failed(_))));
    assert_eq!(outline(&mut a, doc), "A,B,C", "nothing moved");
    for bad in [json!([]), json!([[]]), json!([[0]]), json!([1, 2]), json!("1")] {
        assert!(matches!(a.call("bookmark_indent", &json!({ "doc": doc, "paths": bad })), Err(ToolError::InvalidArgs(_))), "{bad}");
    }
}
