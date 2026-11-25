use super::*;

pub(super) fn unparse(n: &Node) -> String {
    fn sequence(n: &Node, field: &str) -> String {
        items(n, field)
            .iter()
            .map(unparse)
            .collect::<Vec<_>>()
            .join(", ")
    }
    match kind(n) {
        "Name" => s(n, "id").into(),
        "Attribute" => format!("{}.{}", unparse(child(n, "value")), s(n, "attr")),
        "Subscript" => {
            let slice = child(n, "slice");
            let text = if kind(slice) == "Tuple" {
                let text = sequence(slice, "elts");
                if items(slice, "elts").len() == 1 {
                    format!("{text},")
                } else {
                    text
                }
            } else {
                unparse(slice)
            };
            format!("{}[{text}]", unparse(child(n, "value")))
        }
        "Tuple" => {
            let text = sequence(n, "elts");
            format!(
                "({text}{})",
                if items(n, "elts").len() == 1 { "," } else { "" }
            )
        }
        "List" => format!("[{}]", sequence(n, "elts")),
        "Set" => format!("{{{}}}", sequence(n, "elts")),
        "Call" => {
            let mut args = items(n, "args").iter().map(unparse).collect::<Vec<_>>();
            args.extend(items(n, "keywords").iter().map(|kw| {
                if s(kw, "arg").is_empty() {
                    format!("**{}", unparse(child(kw, "value")))
                } else {
                    format!("{}={}", s(kw, "arg"), unparse(child(kw, "value")))
                }
            }));
            format!("{}({})", unparse(child(n, "func")), args.join(", "))
        }
        "Starred" => format!("*{}", unparse(child(n, "value"))),
        "BinOp" => {
            let op = match kind(child(n, "op")) {
                "Add" => "+",
                "Sub" => "-",
                "Mult" => "*",
                "MatMult" => "@",
                "Div" => "/",
                "FloorDiv" => "//",
                "Mod" => "%",
                "Pow" => "**",
                "LShift" => "<<",
                "RShift" => ">>",
                "BitAnd" => "&",
                "BitXor" => "^",
                _ => "|",
            };
            format!(
                "{} {op} {}",
                unparse(child(n, "left")),
                unparse(child(n, "right"))
            )
        }
        "UnaryOp" => format!(
            "{}{}",
            match kind(child(n, "op")) {
                "UAdd" => "+",
                "USub" => "-",
                "Invert" => "~",
                _ => "not ",
            },
            unparse(child(n, "operand"))
        ),
        "Constant" => {
            let v = &n["value"];
            if v.is_null() {
                "None".into()
            } else if let Some(v) = v.as_bool() {
                if v { "True" } else { "False" }.into()
            } else if let Some(v) = v.as_str() {
                let escaped = v
                    .replace('\\', "\\\\")
                    .replace('\n', "\\n")
                    .replace('\r', "\\r")
                    .replace('\t', "\\t");
                if escaped.contains('\'') && !escaped.contains('"') {
                    format!("\"{escaped}\"")
                } else {
                    format!("'{}'", escaped.replace('\'', "\\'"))
                }
            } else if matches!(v, Node::Ellipsis) {
                "...".into()
            } else {
                v.to_string()
            }
        }
        _ => s(n, "_source").into(),
    }
}

fn constant(s: &str) -> bool {
    let s = s.strip_prefix('_').unwrap_or(s);
    s.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

fn immutable<'a>(
    n: &Node,
    constants: &HashMap<&'a str, &'a Node>,
    visiting: &mut Vec<&'a str>,
) -> bool {
    match kind(n) {
        "Constant" => true,
        "Tuple" => items(n, "elts")
            .iter()
            .all(|n| immutable(n, constants, visiting)),
        "UnaryOp" => {
            ["UAdd", "USub"].contains(&kind(child(n, "op")))
                && kind(child(n, "operand")) == "Constant"
                && (child(n, "operand")["value"].is_number()
                    || child(n, "operand")["value"].is_boolean()
                    || matches!(child(n, "operand")["value"], Node::Complex(_)))
        }
        "Name" => {
            let name = s(n, "id");
            if !constant(name) {
                false
            } else if visiting.contains(&name) {
                true
            } else if let Some((&key, &bound)) = constants.get_key_value(name) {
                visiting.push(key);
                let ok = immutable(bound, constants, visiting);
                visiting.pop();
                ok
            } else {
                true
            }
        }
        "Attribute" => constant(s(n, "attr")),
        "Call"
            if [
                "Decimal",
                "Fraction",
                "datetime.timedelta",
                "decimal.Decimal",
                "fractions.Fraction",
                "frozenset",
                "timedelta",
                "tuple",
            ]
            .contains(&unparse(child(n, "func")).as_str()) =>
        {
            items(n, "args")
                .iter()
                .chain(items(n, "keywords").iter().map(|n| child(n, "value")))
                .all(|n| {
                    if ["List", "Set"].contains(&kind(n)) {
                        items(n, "elts")
                            .iter()
                            .all(|n| immutable(n, constants, visiting))
                    } else {
                        immutable(n, constants, visiting)
                    }
                })
        }
        _ => false,
    }
}

pub(super) fn check(file: &FileContext<'_>, out: &mut Vec<Finding>) {
    if file.layer().is_empty() || file.has_layer(&["boundary", "router", "transaction", "test"]) {
        return;
    }
    let mut constants: HashMap<&str, &Node> = HashMap::new();
    for st in items(file.tree, "body") {
        let target = if kind(st) == "Assign" && items(st, "targets").len() == 1 {
            &items(st, "targets")[0]
        } else if kind(st) == "AnnAssign" && !child(st, "value").is_null() {
            child(st, "target")
        } else {
            continue;
        };
        if kind(target) == "Name" {
            constants.insert(s(target, "id"), child(st, "value"));
        }
    }
    let mut visiting = Vec::new();
    visit(
        file,
        file.tree,
        &constants,
        &mut visiting,
        false,
        false,
        out,
    );
}

fn visit<'a>(
    file: &FileContext<'a>,
    n: &'a Node,
    constants: &HashMap<&'a str, &'a Node>,
    visiting: &mut Vec<&'a str>,
    protocol: bool,
    exception: bool,
    out: &mut Vec<Finding>,
) {
    let mut p = protocol;
    let mut e = exception;
    let k = kind(n);
    if k == "ClassDef" {
        for base in items(n, "bases") {
            let b = unparse(base);
            if b == "Protocol" || b.ends_with(".Protocol") {
                p = true;
            }
            if ["Exception", "BaseException"].contains(&b.as_str())
                || b.ends_with(".Exception")
                || b.ends_with(".BaseException")
            {
                e = true;
            }
        }
        e |= s(n, "name").ends_with("Error") || s(n, "name").ends_with("Exception");
    }
    if k == "Lambda"
        || ["FunctionDef", "AsyncFunctionDef"].contains(&k)
            && !p
            && !(e && s(n, "name") == "__init__")
    {
        let a = child(n, "args");
        if items(a, "defaults")
            .iter()
            .chain(items(a, "kw_defaults"))
            .filter(|n| !n.is_null())
            .any(|n| !immutable(n, constants, visiting))
        {
            add(
                out,
                file,
                n,
                "LIT305",
                "parameter defaults must be immutable literals; have the caller pass constructed values",
            );
        }
    }
    for_each_child(n, |c| visit(file, c, constants, visiting, p, e, out));
}
