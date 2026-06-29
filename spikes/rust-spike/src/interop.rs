//! R2 — native-interop falsification test (Rust side).
//! Proves Rust can (a) host a sandboxed WASM tool and (b) use tree-sitter
//! for code-aware symbol selection. We measure how much ceremony each takes.

use anyhow::{Context, Result};

pub fn run() -> Result<()> {
    println!("== R2 native interop (Rust) ==");
    wasm_tool()?;
    treesitter_select()?;
    Ok(())
}

/// Load + call a trivial sandboxed WASM "tool": add(i32,i32)->i32.
fn wasm_tool() -> Result<()> {
    use wasmtime::{Engine, Instance, Module, Store};

    let engine = Engine::default();
    let wasm = wat::parse_str(
        r#"(module (func (export "add") (param i32 i32) (result i32)
              local.get 0 local.get 1 i32.add))"#,
    )?;
    let module = Module::new(&engine, &wasm)?;
    let mut store = Store::new(&engine, ());
    let instance = Instance::new(&mut store, &module, &[])?;
    let add = instance.get_typed_func::<(i32, i32), i32>(&mut store, "add")?;
    let result = add.call(&mut store, (20, 22))?;
    println!("  wasm  : sandboxed tool add(20,22) = {result}  [wasmtime]");
    Ok(())
}

/// Use tree-sitter to find a function's name + byte range — the primitive
/// behind "select a symbol" (object-first verbs ④).
fn treesitter_select() -> Result<()> {
    use tree_sitter::Parser;

    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .context("load rust grammar")?;

    let code = "fn verify(token: &str) -> bool {\n    jwt_validate(token)\n}\n";
    let tree = parser.parse(code, None).context("parse")?;
    let root = tree.root_node();

    if let Some(func) = find_kind(root, "function_item") {
        let name = func
            .child_by_field_name("name")
            .map(|n| &code[n.byte_range()])
            .unwrap_or("<anon>");
        let r = func.byte_range();
        println!(
            "  tree-sitter: fn `{name}` spans bytes {}..{}  [selectable symbol]",
            r.start, r.end
        );
    } else {
        println!("  tree-sitter: no function_item found");
    }
    Ok(())
}

fn find_kind<'a>(node: tree_sitter::Node<'a>, kind: &str) -> Option<tree_sitter::Node<'a>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_kind(child, kind) {
            return Some(found);
        }
    }
    None
}
