// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

fn env() -> Env {
    let mut e = Env::default();
    e.variables.insert(
        "Exporter".into(),
        Type::Struct(BTreeMap::from([("IP".into(), Type::String), ("Name".into(), Type::String)])),
    );
    e.variables.insert("Numbers".into(), Type::Array);
    e
}

fn vars() -> BTreeMap<String, Value> {
    BTreeMap::from([
        (
            "Exporter".into(),
            Value::Map(BTreeMap::from([
                ("IP".into(), Value::from("192.0.2.1")),
                ("Name".into(), Value::from("edge1.paris")),
            ])),
        ),
        ("Numbers".into(), Value::Array(vec![Value::Int(1), Value::Int(2), Value::Int(3)])),
    ])
}

fn eval(src: &str) -> Result<Value, String> {
    Program::compile(src, &env(), false)?.run(&vars(), &mut NoHost)
}

#[test]
fn expressions() {
    let cases: Vec<(&str, Value)> = vec![
        ("1 + 2 * 3", Value::Int(7)),
        ("(1 + 2) * 3", Value::Int(9)),
        ("7 / 2", Value::Float(3.5)),
        ("7 % 4", Value::Int(3)),
        ("2 ** 10", Value::Float(1024.0)),
        ("-3 + 1", Value::Int(-2)),
        ("1 < 2 && 2 <= 2 && 3 > 1.5", Value::Bool(true)),
        ("1 == 1.0", Value::Bool(true)),
        ("\"a\" + 'b' + `c`", Value::from("abc")),
        ("Exporter.Name", Value::from("edge1.paris")),
        ("Exporter[\"IP\"]", Value::from("192.0.2.1")),
        ("Exporter.Name startsWith \"edge\"", Value::Bool(true)),
        ("Exporter.Name endsWith \".paris\"", Value::Bool(true)),
        ("Exporter.Name contains \"1.p\"", Value::Bool(true)),
        ("Exporter.Name matches \"^e.g\"", Value::Bool(true)),
        ("Exporter.Name matches \"^(?i)EDGE\"", Value::Bool(true)),
        ("2 in Numbers", Value::Bool(true)),
        ("5 not in Numbers", Value::Bool(true)),
        ("\"Name\" in Exporter", Value::Bool(true)),
        ("not true", Value::Bool(false)),
        ("!false || false", Value::Bool(true)),
        ("1..3", Value::Array(vec![Value::Int(1), Value::Int(2), Value::Int(3)])),
        ("true ? \"y\" : \"n\"", Value::from("y")),
        ("nil ?? \"default\"", Value::from("default")),
        ("let x = 2; x * 21", Value::Int(42)),
        ("len(Exporter.Name)", Value::Int(11)),
        ("upper(\"ab\")", Value::from("AB")),
        ("split(\"a,b\", \",\")", Value::Array(vec![Value::from("a"), Value::from("b")])),
        ("all(Numbers, {# > 0})", Value::Bool(true)),
        ("any(Numbers, {# > 2})", Value::Bool(true)),
        ("filter(Numbers, {# % 2 == 1})", Value::Array(vec![Value::Int(1), Value::Int(3)])),
        ("map(Numbers, {# * 2})", Value::Array(vec![Value::Int(2), Value::Int(4), Value::Int(6)])),
        ("count(Numbers, {# >= 2})", Value::Int(2)),
        ("Numbers[-1]", Value::Int(3)),
        ("Numbers[1:]", Value::Array(vec![Value::Int(2), Value::Int(3)])),
        ("Exporter.Name[0:4]", Value::from("edge")),
        ("max(1, 5, 3)", Value::Int(5)),
        ("Numbers | sum()", Value::Int(6)),
        ("[1, 2] == [1, 2]", Value::Bool(true)),
        ("{a: 1}.a", Value::Int(1)),
        ("trimPrefix(\"edge1\", \"edge\")", Value::from("1")),
        ("0x10 + 1_000", Value::Int(1016)),
    ];
    for (src, expected) in cases {
        assert_eq!(eval(src), Ok(expected), "{src}");
    }
}

#[test]
fn short_circuit() {
    struct Counting(u32);
    impl Host for Counting {
        fn call(&mut self, _name: &str, _args: Vec<Value>) -> Result<Value, String> {
            self.0 += 1;
            Ok(Value::Bool(true))
        }
    }
    let mut e = env();
    e.functions.insert("F".into(), Signature { params: vec![], variadic: None, returns: Type::Bool });
    let p = Program::compile("false && F()", &e, true).unwrap();
    let mut h = Counting(0);
    assert_eq!(p.run(&vars(), &mut h), Ok(Value::Bool(false)));
    assert_eq!(h.0, 0);
    let p = Program::compile("true && F() && F()", &e, true).unwrap();
    assert_eq!(p.run(&vars(), &mut h), Ok(Value::Bool(true)));
    assert_eq!(h.0, 2);
}

#[test]
fn compile_errors() {
    let mut e = env();
    e.functions
        .insert("Classify".into(), Signature { params: vec![Type::String], variadic: None, returns: Type::Bool });
    for (src, as_bool) in [
        ("Classify(1)", true),
        ("ClassifyStuff(\"x\")", true),
        ("Classify(\"europe\"", true),
        ("Unknown.Name", false),
        ("Exporter.Nope", false),
        ("Exporter.Name > 5", false),
        ("\"x\"", true),
        ("Exporter.Name matches \"(\"", true),
        ("1 &&", false),
        ("Exporter.Name startsWith 1", true),
    ] {
        assert!(Program::compile(src, &e, as_bool).is_err(), "{src} should not compile");
    }
    assert!(Program::compile("Classify(\"europe\")", &e, true).is_ok());
}

#[test]
fn runtime_errors() {
    assert!(eval("Numbers[10]").is_err());
    assert!(eval("1 % 0").is_err());
    assert!(eval("nil.Name").is_err());
    assert_eq!(eval("nil?.Name"), Ok(Value::Nil));
}
