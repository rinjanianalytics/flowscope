// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! The filter grammar, transcribed from `console/filter/parser.peg`, with
//! its actions.

use super::State;
use super::peg::{
    Current, Grammar, Pe, Rule, Value, action, choice, class, ilit, label, lit, not, opt, plus, pred, rule, seq, star,
};
use crate::sqlbuilder::{self as sb, Expr};
use flowscope_schema::{Column, Schema};
use ipnet::IpNet;
use std::net::IpAddr;
use std::rc::Rc;

/// Values built by the actions.
#[derive(Clone, Debug)]
pub enum Node {
    Expr(Option<Expr>),
    Column(Column),
    Uint(u64),
    Addr(IpAddr),
    Prefix(IpNet),
    Str(String),
    /// Operator and right side of a condition.
    Cond(String, Expr),
}

type V = Value<Node>;
type C<'a> = Current<'a, Node, State>;
type P = Pe<Node, State>;

fn custom(n: Node) -> V {
    Value::Custom(n)
}

fn expr_of(v: Option<&V>) -> Option<Expr> {
    match v {
        Some(Value::Custom(Node::Expr(e))) => e.clone(),
        _ => None,
    }
}

fn node(c: &C, name: &str) -> Option<Node> {
    match c.label(name) {
        Some(Value::Custom(n)) => Some(n.clone()),
        _ => None,
    }
}

fn label_text(c: &C, name: &str) -> String {
    match c.label(name) {
        Some(Value::Text(t)) => t.clone(),
        Some(Value::Custom(Node::Str(s))) => s.clone(),
        Some(Value::List(l)) => l.iter().map(|v| v.text().to_string()).collect(),
        _ => String::new(),
    }
}

fn col(c: &C, name: &str) -> Column {
    match node(c, name) {
        Some(Node::Column(col)) => col,
        _ => Column::default(),
    }
}

fn items(c: &C, name: &str) -> Vec<Node> {
    fn flatten(v: &V, out: &mut Vec<Node>) {
        match v {
            Value::Custom(n) => out.push(n.clone()),
            Value::List(l) => l.iter().for_each(|i| flatten(i, out)),
            _ => {}
        }
    }
    let mut out = vec![];
    if let Some(v) = c.label(name) {
        flatten(v, &mut out);
    }
    out
}

/// The opposite column (Src ↔ Dst, In ↔ Out), when it exists and is
/// enabled.
pub fn opposite_column(schema: &Schema, col: &Column) -> Column {
    let name = &col.name;
    let candidate = if let Some(r) = name.strip_prefix("Src") {
        format!("Dst{r}")
    } else if let Some(r) = name.strip_prefix("Dst") {
        format!("Src{r}")
    } else if let Some(r) = name.strip_prefix("In") {
        format!("Out{r}")
    } else if let Some(r) = name.strip_prefix("Out") {
        format!("In{r}")
    } else {
        String::new()
    };
    match schema.lookup_column_by_name(&candidate) {
        Some(c) if !c.disabled => c.clone(),
        _ => col.clone(),
    }
}

fn reverse_column(state: &State, col: &Column) -> Column {
    if state.reverse { opposite_column(&state.schema, col) } else { col.clone() }
}

/// The SQL for a column, in the right direction.
fn column_expr(state: &mut State, col: &Column) -> Expr {
    let col = reverse_column(state, col);
    if col.clickhouse_main_only {
        state.main_table_required = true;
    }
    sb::column(&col.name)
}

fn value_expr(state: &mut State, n: &Node) -> Expr {
    match n {
        Node::Column(c) => column_expr(state, c),
        Node::Uint(u) => sb::uint(*u),
        Node::Expr(Some(e)) => e.clone(),
        _ => sb::uint(0),
    }
}

fn find_column<'a>(state: &'a State, name: &str) -> Option<&'a Column> {
    state.schema.columns().find(|c| c.name.eq_ignore_ascii_case(name))
}

fn is_type(types: &'static [&'static str]) -> P {
    pred(move |c: &mut C| {
        let name = label_text(c, "value");
        let name = if name.is_empty() { label_text(c, "column") } else { name };
        Ok(find_column(c.state, &name).is_some_and(|col| types.contains(&col.parser_type)))
    })
}

fn accept_column(c: &mut C) -> Result<V, String> {
    let Some(col) = find_column(c.state, c.text).cloned() else {
        return Err(format!("unknown column {:?}", c.text));
    };
    if let Some(positions) = &mut c.state.columns {
        positions.insert(c.offset, (c.text.len(), col.clone()));
    }
    Ok(custom(Node::Column(col)))
}

/// `value:ColumnName &{ is of type } { accept }`
fn typed_column(types: &'static [&'static str]) -> P {
    action(seq(vec![label("value", rule("ColumnName")), is_type(types)]), accept_column)
}

fn ws() -> P {
    rule("_")
}

fn op_choice(ops: &[&str]) -> P {
    choice(ops.iter().map(|o| lit(o)).collect())
}

fn apply_cond(left: Expr, cond: Option<Node>) -> Expr {
    match cond {
        Some(Node::Cond(op, right)) => sb::op(left, &op, right),
        _ => left,
    }
}

/// `column:<typed column> _ rcond:<rule>` applying the condition.
fn condition_with(types: &'static [&'static str], rcond: &'static str) -> P {
    action(seq(vec![label("column", typed_column(types)), ws(), label("rcond", rule(rcond))]), move |c: &mut C| {
        let column = col(c, "column");
        let left = column_expr(c.state, &column);
        let cond = node(c, "rcond");
        Ok(custom(Node::Expr(Some(apply_cond(left, cond)))))
    })
}

fn cond_value(op: String, value: Expr) -> V {
    custom(Node::Cond(op, value))
}

/// The last IP of a subnet.
fn last_ip(net: &IpNet) -> IpAddr {
    match net {
        IpNet::V4(n) => IpAddr::V4(n.broadcast()),
        IpNet::V6(n) => IpAddr::V6(n.broadcast()),
    }
}

fn to_ipv6(addr: IpAddr) -> Expr {
    sb::function("toIPv6", vec![sb::string(&addr.to_string())])
}

pub fn between_prefix(left: Expr, prefix: &IpNet, negate: bool) -> Expr {
    sb::between(left, to_ipv6(prefix.trunc().addr()), to_ipv6(last_ip(prefix)), negate)
}

fn require_main_table(state: &mut State, names: &[&str]) {
    for n in names {
        if let Some(c) = state.schema.lookup_column_by_name(n)
            && c.clickhouse_main_only
        {
            state.main_table_required = true;
        }
    }
}

fn prefix_condition(state: &mut State, col: &Column, prefix: &IpNet) -> Expr {
    let col = reverse_column(state, col);
    if col.clickhouse_materialized {
        if col.clickhouse_main_only {
            state.main_table_required = true;
        }
        return sb::op(sb::column(&col.name), "=", sb::string(&prefix.to_string()));
    }
    let direction = if col.name.starts_with("Src") { "Src" } else { "Dst" };
    let (addr, mask) = (format!("{direction}Addr"), format!("{direction}NetMask"));
    require_main_table(state, &[&addr, &mask]);
    sb::and(vec![
        between_prefix(sb::column(&addr), prefix, false),
        sb::op(sb::column(&mask), "=", sb::uint(prefix.prefix_len() as u64)),
    ])
    .unwrap()
}

fn ip_or_subnet_condition(state: &mut State, col: &Column, operator: &str, items: &[Node]) -> Result<Expr, String> {
    let left = column_expr(state, col);
    let mut addresses = vec![];
    let mut conditions = vec![];
    for item in items {
        match item {
            Node::Addr(a) => addresses.push(to_ipv6(*a)),
            Node::Prefix(p) => conditions.push(between_prefix(left.clone(), p, false)),
            _ => return Err("expecting an IP address or a subnet".into()),
        }
    }
    if !addresses.is_empty() {
        conditions.insert(0, sb::op(left, "IN", sb::tuple(addresses)));
    }
    let n = conditions.len();
    let expr = sb::or(conditions).unwrap_or_else(|| sb::tuple(vec![]));
    if operator == "NOT IN" {
        return Ok(sb::not(expr));
    }
    Ok(if n > 1 { sb::parens(expr) } else { expr })
}

fn mac_to_num(mac: &str) -> Expr {
    sb::function("MACStringToNum", vec![sb::string(mac)])
}

/// Parse a MAC address like Go's net.ParseMAC, for the forms the grammar
/// allows (colons or dots). Returns the address formatted with colons.
fn parse_mac(s: &str) -> Result<Vec<u8>, ()> {
    let hex = |p: &str| u8::from_str_radix(p, 16).map_err(|_| ());
    if s.len() < 14 {
        return Err(());
    }
    let bytes: Vec<u8> = if s.as_bytes()[2] == b':' {
        if !(s.len() + 1).is_multiple_of(3) {
            return Err(());
        }
        let parts: Vec<&str> = s.split(':').collect();
        if parts.iter().any(|p| p.len() != 2) {
            return Err(());
        }
        parts.iter().map(|p| hex(p)).collect::<Result<_, _>>()?
    } else if s.as_bytes()[4] == b'.' {
        if !(s.len() + 1).is_multiple_of(5) {
            return Err(());
        }
        let parts: Vec<&str> = s.split('.').collect();
        if parts.iter().any(|p| p.len() != 4) {
            return Err(());
        }
        let mut out = vec![];
        for p in parts {
            out.push(hex(&p[..2])?);
            out.push(hex(&p[2..])?);
        }
        out
    } else {
        return Err(());
    };
    match bytes.len() {
        6 | 8 | 20 => Ok(bytes),
        _ => Err(()),
    }
}

fn large_community(v1: u64, v2: u64, v3: u64) -> Expr {
    let shift =
        |v: u64, bits: u64| sb::function("bitShiftLeft", vec![sb::cast(sb::uint(v), "UInt128"), sb::uint(bits)]);
    sb::op(sb::op(shift(v1, 64), "+", shift(v2, 32)), "+", sb::cast(sb::uint(v3), "UInt128"))
}

fn large_communities_column(state: &State, col: &Column) -> Column {
    let name = col.name.replacen("Communities", "LargeCommunities", 1);
    state.schema.lookup_column_by_name(&name).cloned().unwrap_or_default()
}

fn protocol_name(database: &str, proto: Expr) -> Expr {
    let dictionary = format!("{}.protocols", sb::ident(database));
    sb::function("dictGetOrDefault", vec![sb::string(&dictionary), sb::string("name"), proto, sb::string("???")])
}

/// Combine `head (_ KW _ item)*` with AND or OR.
fn combine(c: &C, join: fn(Vec<Expr>) -> Option<Expr>) -> V {
    let mut exprs = vec![];
    if let Some(e) = expr_of(c.label("head")) {
        exprs.push(e);
    }
    if let Some(Value::List(rest)) = c.label("rest") {
        for item in rest {
            if let Some(e) = expr_of(item.list().last()) {
                exprs.push(e);
            }
        }
    }
    custom(Node::Expr(join(exprs)))
}

fn unsigned(bits: u32, message: &'static str) -> P {
    action(seq(vec![plus(class("[0-9]")), not(rule("IdentStart"))]), move |c: &mut C| {
        let max = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
        match c.text.parse::<u64>() {
            Ok(v) if v <= max => Ok(custom(Node::Uint(v))),
            _ => Err(message.into()),
        }
    })
}

/// `head:Item _ ',' _ tail:List / value:Item`
fn list(item: &'static str, this: &'static str) -> P {
    choice(vec![
        action(seq(vec![label("head", rule(item)), ws(), lit(","), ws(), label("tail", rule(this))]), |c: &mut C| {
            let mut out = vec![c.label("head").cloned().unwrap_or_default()];
            out.extend(c.label("tail").map(|t| t.list().to_vec()).unwrap_or_default());
            Ok(Value::List(out))
        }),
        action(label("value", rule(item)), |c: &mut C| {
            Ok(Value::List(vec![c.label("value").cloned().unwrap_or_default()]))
        }),
    ])
}

/// `( '"' chars '"' / "'" chars "'" ) / unterminated`
fn string_literal() -> P {
    let body = |q: &'static str, char_rule: &'static str| {
        action(seq(vec![lit(q), label("chars", star(rule(char_rule))), lit(q)]), |c: &mut C| {
            let mut out = String::new();
            for ch in c.label("chars").map(|v| v.list().to_vec()).unwrap_or_default() {
                let s = ch.text();
                if s.len() == 2 && s.starts_with('\\') {
                    out.push_str(&s[1..]);
                } else {
                    out.push_str(s);
                }
            }
            Ok(custom(Node::Str(out)))
        })
    };
    let eol_or_eof = || choice(vec![rule("EOL"), rule("EOF")]);
    choice(vec![
        choice(vec![body("\"", "DoubleStringChar"), body("'", "SingleStringChar")]),
        action(
            choice(vec![
                seq(vec![lit("\""), star(rule("DoubleStringChar")), eol_or_eof()]),
                seq(vec![lit("'"), star(rule("SingleStringChar")), eol_or_eof()]),
            ]),
            |_| Err("string literal not terminated".into()),
        ),
    ])
}

fn string_char(quote: &'static str) -> P {
    action(
        choice(vec![
            seq(vec![lit("\\"), choice(vec![lit("\\"), lit(quote)])]),
            seq(vec![not(choice(vec![lit(quote), rule("EOL")])), rule("SourceChar")]),
        ]),
        |c: &mut C| Ok(Value::Text(c.text.to_string())),
    )
}

fn keyword(word: &'static str, value: &'static str) -> P {
    action(seq(vec![ilit(word), not(rule("IdentStart"))]), move |_| Ok(Value::Text(value.into())))
}

/// The operator text of a matched operator (literal or keyword).
fn op_text(c: &C, name: &str) -> String {
    label_text(c, name)
}

fn expr_value(e: Expr) -> V {
    custom(Node::Expr(Some(e)))
}

/// Build the grammar.
pub fn grammar() -> Grammar<Node, State> {
    let rules: Vec<(&'static str, P)> = vec![
        (
            "Input",
            action(seq(vec![ws(), label("expr", rule("Expr")), ws(), rule("EOF")]), |c: &mut C| {
                Ok(c.label("expr").cloned().unwrap_or_default())
            }),
        ),
        ("Expr", rule("OrExpr")),
        (
            "OrExpr",
            action(
                seq(vec![
                    label("head", rule("AndExpr")),
                    label("rest", star(seq(vec![ws(), rule("KW_OR"), ws(), rule("AndExpr")]))),
                ]),
                |c: &mut C| Ok(combine(c, sb::or)),
            ),
        ),
        (
            "AndExpr",
            action(
                seq(vec![
                    label("head", rule("UnitExpr")),
                    label("rest", star(seq(vec![ws(), rule("KW_AND"), ws(), rule("UnitExpr")]))),
                ]),
                |c: &mut C| Ok(combine(c, sb::and)),
            ),
        ),
        ("UnitExpr", choice(vec![rule("SubExpr"), rule("NotExpr"), rule("ConditionExpr")])),
        (
            "SubExpr",
            action(seq(vec![lit("("), ws(), label("expr", rule("Expr")), ws(), lit(")")]), |c: &mut C| {
                Ok(custom(Node::Expr(expr_of(c.label("expr")).map(sb::parens))))
            }),
        ),
        (
            "NotExpr",
            action(seq(vec![rule("KW_NOT"), ws(), label("expr", rule("UnitExpr"))]), |c: &mut C| {
                Ok(custom(Node::Expr(expr_of(c.label("expr")).map(sb::not))))
            }),
        ),
        (
            "ConditionExpr",
            Pe::Recovery {
                expr: Box::new(choice(vec![
                    rule("ConditionIPExpr"),
                    rule("ConditionPrefixExpr"),
                    rule("ConditionMACExpr"),
                    rule("ConditionStringExpr"),
                    rule("ConditionBoundaryExpr"),
                    rule("ConditionUintExpr"),
                    rule("ConditionArrayUintExpr"),
                    rule("ConditionASExpr"),
                    rule("ConditionASPathExpr"),
                    rule("ConditionCommunitiesExpr"),
                    rule("ConditionETypeExpr"),
                    rule("ConditionProtoExpr"),
                    rule("ConditionDirectionExpr"),
                    seq(vec![not(rule("ColumnName")), Pe::Throw("errColumnName")]),
                ])),
                labels: vec!["errColumnName"],
                recover: Box::new(rule("ErrColumnName")),
            },
        ),
        ("ErrColumnName", Pe::StateCode(Rc::new(|_| Err("expecting a column name".into())))),
        (
            "ColumnName",
            action(seq(vec![plus(class("[A-Za-z0-9]")), not(rule("IdentStart"))]), |c: &mut C| {
                Ok(custom(Node::Str(c.text.to_string())))
            }),
        ),
        ("ColumnIP", typed_column(&["ip"])),
        (
            "ConditionIPExpr",
            choice(vec![
                action(
                    seq(vec![
                        label("column", rule("ColumnIP")),
                        ws(),
                        choice(vec![lit("<<"), lit("=")]),
                        ws(),
                        label("subnet", rule("Subnet")),
                    ]),
                    |c: &mut C| {
                        let column = col(c, "column");
                        let left = column_expr(c.state, &column);
                        match node(c, "subnet") {
                            Some(Node::Prefix(p)) => Ok(expr_value(between_prefix(left, &p, false))),
                            _ => Ok(custom(Node::Expr(None))),
                        }
                    },
                ),
                action(
                    seq(vec![
                        label("column", rule("ColumnIP")),
                        ws(),
                        choice(vec![lit("!<<"), lit("!=")]),
                        ws(),
                        label("subnet", rule("Subnet")),
                    ]),
                    |c: &mut C| {
                        let column = col(c, "column");
                        let left = column_expr(c.state, &column);
                        match node(c, "subnet") {
                            Some(Node::Prefix(p)) => Ok(expr_value(between_prefix(left, &p, true))),
                            _ => Ok(custom(Node::Expr(None))),
                        }
                    },
                ),
                action(
                    seq(vec![
                        label("column", rule("ColumnIP")),
                        ws(),
                        label("operator", op_choice(&["=", "!="])),
                        ws(),
                        label("ip", rule("IP")),
                    ]),
                    |c: &mut C| {
                        let column = col(c, "column");
                        let left = column_expr(c.state, &column);
                        let op = op_text(c, "operator");
                        let ip = match node(c, "ip") {
                            Some(Node::Addr(a)) => a.to_string(),
                            _ => "invalid IP".into(),
                        };
                        Ok(expr_value(sb::op(left, &op, sb::function("toIPv6", vec![sb::string(&ip)]))))
                    },
                ),
                action(
                    seq(vec![
                        label("column", rule("ColumnIP")),
                        ws(),
                        label("operator", rule("InOperator")),
                        ws(),
                        lit("("),
                        ws(),
                        label("value", rule("ListIPOrSubnet")),
                        ws(),
                        lit(")"),
                    ]),
                    |c: &mut C| {
                        let column = col(c, "column");
                        let op = op_text(c, "operator");
                        let values = items(c, "value");
                        ip_or_subnet_condition(c.state, &column, &op, &values).map(expr_value)
                    },
                ),
            ]),
        ),
        (
            "ConditionPrefixExpr",
            action(
                seq(vec![
                    label("column", typed_column(&["prefix"])),
                    ws(),
                    label("operator", op_choice(&["=", "!="])),
                    ws(),
                    label("prefix", rule("Prefix")),
                ]),
                |c: &mut C| {
                    let column = col(c, "column");
                    let p = match node(c, "prefix") {
                        Some(Node::Prefix(p)) => p,
                        // Invalid prefix: the parse fails anyway.
                        _ => IpNet::default(),
                    };
                    let expr = prefix_condition(c.state, &column, &p);
                    Ok(expr_value(if op_text(c, "operator") == "!=" { sb::not(expr) } else { expr }))
                },
            ),
        ),
        ("ConditionMACExpr", condition_with(&["mac"], "RConditionMACExpr")),
        (
            "RConditionMACExpr",
            choice(vec![
                action(
                    seq(vec![label("operator", op_choice(&["=", "!="])), ws(), label("mac", rule("MAC"))]),
                    |c: &mut C| Ok(cond_value(op_text(c, "operator"), mac_to_num(&label_text(c, "mac")))),
                ),
                action(
                    seq(vec![
                        label("operator", rule("InOperator")),
                        ws(),
                        lit("("),
                        ws(),
                        label("value", rule("ListMAC")),
                        ws(),
                        lit(")"),
                    ]),
                    |c: &mut C| {
                        let macs: Vec<Expr> = items(c, "value")
                            .iter()
                            .map(|n| match n {
                                Node::Str(s) => mac_to_num(s),
                                _ => mac_to_num(""),
                            })
                            .collect();
                        Ok(cond_value(op_text(c, "operator"), sb::tuple(macs)))
                    },
                ),
            ]),
        ),
        ("ColumnString", typed_column(&["string"])),
        ("ConditionStringExpr", condition_with(&["string"], "RConditionStringExpr")),
        (
            "RConditionStringExpr",
            choice(vec![
                action(
                    seq(vec![
                        label("operator", choice(vec![lit("="), lit("!="), rule("LikeOperator")])),
                        ws(),
                        label("str", rule("StringLiteral")),
                    ]),
                    |c: &mut C| Ok(cond_value(op_text(c, "operator"), sb::string(&label_text(c, "str")))),
                ),
                action(
                    seq(vec![label("operator", op_choice(&["=", "!="])), ws(), label("value", rule("ColumnString"))]),
                    |c: &mut C| {
                        let column = col(c, "value");
                        let right = column_expr(c.state, &column);
                        Ok(cond_value(op_text(c, "operator"), right))
                    },
                ),
                action(
                    seq(vec![
                        label("operator", rule("InOperator")),
                        ws(),
                        lit("("),
                        ws(),
                        label("value", rule("ListString")),
                        ws(),
                        lit(")"),
                    ]),
                    |c: &mut C| {
                        let strings: Vec<Expr> = items(c, "value")
                            .iter()
                            .map(|n| match n {
                                Node::Str(s) => sb::string(s),
                                _ => sb::string(""),
                            })
                            .collect();
                        Ok(cond_value(op_text(c, "operator"), sb::tuple(strings)))
                    },
                ),
            ]),
        ),
        (
            "ConditionBoundaryExpr",
            action(
                seq(vec![
                    label("column", typed_column(&["boundary"])),
                    ws(),
                    label("operator", op_choice(&["=", "!="])),
                    ws(),
                    label("boundary", choice(vec![ilit("external"), ilit("internal"), ilit("undefined")])),
                ]),
                |c: &mut C| {
                    let column = col(c, "column");
                    let left = column_expr(c.state, &column);
                    Ok(expr_value(sb::op(
                        left,
                        &op_text(c, "operator"),
                        sb::string(&label_text(c, "boundary").to_lowercase()),
                    )))
                },
            ),
        ),
        ("ColumnUint", typed_column(&["uint"])),
        ("ConditionUintExpr", condition_with(&["uint"], "RConditionUintExpr")),
        (
            "RConditionUintExpr",
            choice(vec![
                action(
                    seq(vec![
                        label("operator", op_choice(&["=", ">=", "<=", "<", ">", "!="])),
                        ws(),
                        label("value", choice(vec![rule("Unsigned64"), rule("ColumnUint")])),
                    ]),
                    |c: &mut C| {
                        let v = node(c, "value").map(|n| value_expr(c.state, &n)).unwrap_or(sb::uint(0));
                        Ok(cond_value(op_text(c, "operator"), v))
                    },
                ),
                action(
                    seq(vec![
                        label("operator", rule("InOperator")),
                        ws(),
                        lit("("),
                        ws(),
                        label("value", rule("ListUnsigned64")),
                        ws(),
                        lit(")"),
                    ]),
                    |c: &mut C| {
                        let values: Vec<Expr> = items(c, "value").iter().map(|n| value_expr(c.state, n)).collect();
                        Ok(cond_value(op_text(c, "operator"), sb::tuple(values)))
                    },
                ),
            ]),
        ),
        (
            "ConditionArrayUintExpr",
            choice(vec![
                has_condition(&["array(uint)"], "Unsigned64", false, false),
                has_condition(&["array(uint)"], "Unsigned64", true, false),
            ]),
        ),
        ("ColumnASN", typed_column(&["asn"])),
        ("ConditionASExpr", condition_with(&["asn"], "RConditionASExpr")),
        (
            "RConditionASExpr",
            choice(vec![
                action(
                    seq(vec![
                        label("operator", op_choice(&["=", "!="])),
                        ws(),
                        label("value", choice(vec![rule("ASN"), rule("ColumnASN")])),
                    ]),
                    |c: &mut C| {
                        let v = node(c, "value").map(|n| value_expr(c.state, &n)).unwrap_or(sb::uint(0));
                        Ok(cond_value(op_text(c, "operator"), v))
                    },
                ),
                action(
                    seq(vec![
                        label("operator", rule("InOperator")),
                        ws(),
                        lit("("),
                        ws(),
                        label("value", rule("ListASN")),
                        ws(),
                        lit(")"),
                    ]),
                    |c: &mut C| {
                        let values: Vec<Expr> = items(c, "value").iter().map(|n| value_expr(c.state, n)).collect();
                        Ok(cond_value(op_text(c, "operator"), sb::tuple(values)))
                    },
                ),
            ]),
        ),
        (
            "ConditionASPathExpr",
            choice(vec![
                has_condition(&["aspath"], "ASN", false, false),
                has_condition(&["aspath"], "ASN", true, false),
            ]),
        ),
        (
            "ConditionCommunitiesExpr",
            choice(vec![
                has_condition(&["community"], "Community", false, false),
                has_condition(&["community"], "Community", true, false),
                has_condition(&["community"], "LargeCommunity", false, true),
                has_condition(&["community"], "LargeCommunity", true, true),
            ]),
        ),
        (
            "ConditionETypeExpr",
            action(
                seq(vec![
                    label("column", typed_column(&["etype"])),
                    ws(),
                    label("operator", op_choice(&["=", "!="])),
                    ws(),
                    label("value", choice(vec![ilit("IPv4"), ilit("IPv6")])),
                ]),
                |c: &mut C| {
                    let column = col(c, "column");
                    let left = column_expr(c.state, &column);
                    let etype = if label_text(c, "value").eq_ignore_ascii_case("ipv4") { 0x800 } else { 0x86dd };
                    Ok(expr_value(sb::op(left, &op_text(c, "operator"), sb::uint(etype))))
                },
            ),
        ),
        ("ConditionProtoExpr", choice(vec![rule("ConditionProtoIntExpr"), rule("ConditionProtoStrExpr")])),
        ("ConditionProtoIntExpr", condition_with(&["proto"], "RConditionProtoIntExpr")),
        (
            "RConditionProtoIntExpr",
            choice(vec![
                action(
                    seq(vec![
                        label("operator", op_choice(&["=", ">=", "<=", "<", ">", "!="])),
                        ws(),
                        label("value", rule("Unsigned8")),
                    ]),
                    |c: &mut C| {
                        let v = node(c, "value").map(|n| value_expr(c.state, &n)).unwrap_or(sb::uint(0));
                        Ok(cond_value(op_text(c, "operator"), v))
                    },
                ),
                action(
                    seq(vec![
                        label("operator", rule("InOperator")),
                        ws(),
                        lit("("),
                        ws(),
                        label("value", rule("ListUnsigned8")),
                        ws(),
                        lit(")"),
                    ]),
                    |c: &mut C| {
                        let values: Vec<Expr> = items(c, "value").iter().map(|n| value_expr(c.state, n)).collect();
                        Ok(cond_value(op_text(c, "operator"), sb::tuple(values)))
                    },
                ),
            ]),
        ),
        (
            "ConditionProtoStrExpr",
            action(
                seq(vec![
                    label("column", typed_column(&["proto"])),
                    ws(),
                    label("rcond", rule("RConditionProtoStrExpr")),
                ]),
                |c: &mut C| {
                    let column = col(c, "column");
                    let left = column_expr(c.state, &column);
                    let left = protocol_name(&c.state.database, left);
                    let cond = node(c, "rcond");
                    Ok(expr_value(apply_cond(left, cond)))
                },
            ),
        ),
        (
            "RConditionProtoStrExpr",
            choice(vec![
                action(
                    seq(vec![label("operator", op_choice(&["=", "!="])), ws(), label("value", rule("StringLiteral"))]),
                    |c: &mut C| Ok(cond_value(op_text(c, "operator"), sb::string(&label_text(c, "value")))),
                ),
                action(
                    seq(vec![
                        label("operator", rule("InOperator")),
                        ws(),
                        lit("("),
                        ws(),
                        label("value", rule("ListString")),
                        ws(),
                        lit(")"),
                    ]),
                    |c: &mut C| {
                        let strings: Vec<Expr> = items(c, "value")
                            .iter()
                            .map(|n| match n {
                                Node::Str(s) => sb::string(s),
                                _ => sb::string(""),
                            })
                            .collect();
                        Ok(cond_value(op_text(c, "operator"), sb::tuple(strings)))
                    },
                ),
            ]),
        ),
        (
            "ConditionDirectionExpr",
            action(
                seq(vec![
                    label("column", typed_column(&["direction"])),
                    ws(),
                    label("operator", op_choice(&["=", "!="])),
                    ws(),
                    label("direction", choice(vec![ilit("undefined"), ilit("ingress"), ilit("egress")])),
                ]),
                |c: &mut C| {
                    let column = col(c, "column");
                    let left = column_expr(c.state, &column);
                    Ok(expr_value(sb::op(
                        left,
                        &op_text(c, "operator"),
                        sb::string(&label_text(c, "direction").to_lowercase()),
                    )))
                },
            ),
        ),
        (
            "IP",
            action(seq(vec![plus(class("[0-9A-Fa-f:.]")), not(rule("IdentStart"))]), |c: &mut C| {
                c.text.parse::<IpAddr>().map(|a| custom(Node::Addr(a))).map_err(|_| "expecting an IP address".into())
            }),
        ),
        (
            "Subnet",
            action(
                seq(vec![plus(class("[0-9A-Fa-f:.]")), lit("/"), plus(class("[0-9]")), not(rule("IdentStart"))]),
                |c: &mut C| {
                    parse_prefix(c.text).map(|p| custom(Node::Prefix(p))).ok_or_else(|| "expecting a subnet".into())
                },
            ),
        ),
        (
            "ListIPOrSubnet",
            choice(vec![
                action(
                    seq(vec![
                        label("head", choice(vec![rule("Subnet"), rule("IP")])),
                        ws(),
                        lit(","),
                        ws(),
                        label("tail", rule("ListIPOrSubnet")),
                    ]),
                    |c: &mut C| {
                        let mut out = vec![c.label("head").cloned().unwrap_or_default()];
                        out.extend(c.label("tail").map(|t| t.list().to_vec()).unwrap_or_default());
                        Ok(Value::List(out))
                    },
                ),
                action(label("value", choice(vec![rule("Subnet"), rule("IP")])), |c: &mut C| {
                    Ok(Value::List(vec![c.label("value").cloned().unwrap_or_default()]))
                }),
            ]),
        ),
        (
            "Prefix",
            action(
                seq(vec![plus(class("[0-9A-Fa-f:.]")), lit("/"), plus(class("[0-9]")), not(rule("IdentStart"))]),
                |c: &mut C| {
                    parse_prefix(c.text).map(|p| custom(Node::Prefix(p))).ok_or_else(|| "expecting a prefix".into())
                },
            ),
        ),
        (
            "MAC",
            action(seq(vec![plus(class("[0-9A-Fa-f:.]")), not(rule("IdentStart"))]), |c: &mut C| {
                match parse_mac(c.text) {
                    Err(()) => Err("expecting a MAC address".into()),
                    Ok(b) if b.len() != 6 => Err("expecting a 6-byte MAC address".into()),
                    Ok(b) => Ok(custom(Node::Str(b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(":")))),
                }
            }),
        ),
        ("ListMAC", list("MAC", "ListMAC")),
        (
            "ASN",
            action(
                seq(vec![opt(ilit("AS")), label("value", rule("Unsigned32")), not(rule("IdentStart"))]),
                |c: &mut C| Ok(c.label("value").cloned().unwrap_or_default()),
            ),
        ),
        ("ListASN", list("ASN", "ListASN")),
        (
            "Community",
            action(
                seq(vec![
                    label("value1", rule("Unsigned16")),
                    lit(":"),
                    label("value2", rule("Unsigned16")),
                    not(rule("IdentStart")),
                    not(lit(":")),
                ]),
                |c: &mut C| {
                    let (Some(Node::Uint(a)), Some(Node::Uint(b))) = (node(c, "value1"), node(c, "value2")) else {
                        return Ok(custom(Node::Uint(0)));
                    };
                    Ok(custom(Node::Uint((a << 16) + b)))
                },
            ),
        ),
        (
            "LargeCommunity",
            action(
                seq(vec![
                    label("value1", rule("Unsigned32")),
                    lit(":"),
                    label("value2", rule("Unsigned32")),
                    lit(":"),
                    label("value3", rule("Unsigned32")),
                    not(rule("IdentStart")),
                    not(lit(":")),
                ]),
                |c: &mut C| {
                    let get = |n: &str| match node(c, n) {
                        Some(Node::Uint(v)) => v,
                        _ => 0,
                    };
                    Ok(expr_value(large_community(get("value1"), get("value2"), get("value3"))))
                },
            ),
        ),
        ("StringLiteral", string_literal()),
        ("SourceChar", Pe::Any),
        ("DoubleStringChar", string_char("\"")),
        ("SingleStringChar", string_char("'")),
        ("ListString", list("StringLiteral", "ListString")),
        ("Unsigned8", unsigned(8, "expecting an unsigned 8-bit integer")),
        ("ListUnsigned8", list("Unsigned8", "ListUnsigned8")),
        ("Unsigned16", unsigned(16, "expecting an unsigned 16-bit integer")),
        ("Unsigned32", unsigned(32, "expecting an unsigned 32-bit integer")),
        ("Unsigned64", unsigned(64, "expecting an unsigned integer")),
        ("ListUnsigned64", list("Unsigned64", "ListUnsigned64")),
        ("LikeOperator", choice(vec![rule("KW_LIKE"), rule("KW_ILIKE"), rule("KW_UNLIKE"), rule("KW_IUNLIKE")])),
        ("InOperator", choice(vec![rule("KW_IN"), rule("KW_NOTIN")])),
        ("KW_AND", keyword("AND", "AND")),
        ("KW_OR", keyword("OR", "OR")),
        ("KW_NOT", keyword("NOT", "NOT")),
        ("KW_LIKE", keyword("LIKE", "LIKE")),
        ("KW_ILIKE", keyword("ILIKE", "ILIKE")),
        ("KW_IN", keyword("IN", "IN")),
        ("KW_UNLIKE", keyword("UNLIKE", "NOT LIKE")),
        ("KW_IUNLIKE", keyword("IUNLIKE", "NOT ILIKE")),
        ("KW_NOTIN", keyword("NOTIN", "NOT IN")),
        ("SingleLineComment", seq(vec![lit("--"), star(seq(vec![not(rule("EOL")), rule("SourceChar")]))])),
        (
            "MultiLineComment",
            seq(vec![
                lit("/*"),
                star(seq(vec![not(lit("*/")), rule("SourceChar")])),
                choice(vec![lit("*/"), action(rule("EOF"), |_| Err("comment unclosed".into()))]),
            ]),
        ),
        ("IdentStart", class("[A-Za-z_]")),
        ("_", star(choice(vec![class("[ \\n\\r\\t]"), rule("SingleLineComment"), rule("MultiLineComment")]))),
        ("EOF", not(Pe::Any)),
        ("EOL", lit("\n")),
    ];
    Grammar::new(rules.into_iter().map(|(name, expr)| Rule { name, expr }).collect())
}

/// `column:<typed> _ "=" _ value:<rule>` → has(column, value), negated for "!=".
fn has_condition(types: &'static [&'static str], value_rule: &'static str, negate: bool, large: bool) -> P {
    action(
        seq(vec![
            label("column", typed_column(types)),
            ws(),
            lit(if negate { "!=" } else { "=" }),
            ws(),
            label("value", rule(value_rule)),
        ]),
        move |c: &mut C| {
            let mut column = col(c, "column");
            if large {
                column = large_communities_column(c.state, &column);
            }
            let left = column_expr(c.state, &column);
            let value = node(c, "value").map(|n| value_expr(c.state, &n)).unwrap_or(sb::uint(0));
            let has = sb::function("has", vec![left, value]);
            Ok(expr_value(if negate { sb::not(has) } else { has }))
        },
    )
}

/// Parse a prefix like Go's netip.ParsePrefix (the address is kept as is).
fn parse_prefix(s: &str) -> Option<IpNet> {
    let (addr, bits) = s.split_once('/')?;
    if bits.len() > 1 && bits.starts_with('0') {
        return None;
    }
    let addr: IpAddr = addr.parse().ok()?;
    let bits: u8 = bits.parse().ok()?;
    IpNet::new(addr, bits).ok()
}
