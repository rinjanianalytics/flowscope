// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Port of `outlet/core/classifier_test.go`.

use super::*;

fn exporter(ip: &str, name: &str) -> ExporterInfo {
    ExporterInfo { ip: ip.into(), name: name.into() }
}

#[test]
fn exporter_classifier() {
    let ec = |f: fn(&mut ExporterClassification)| {
        let mut c = ExporterClassification::default();
        f(&mut c);
        Some(c)
    };
    let cases: Vec<(&str, ExporterInfo, Option<ExporterClassification>)> = vec![
        ("false", ExporterInfo::default(), ec(|_| {})),
        (r#"Classify("europe")"#, ExporterInfo::default(), ec(|c| c.group = "europe".into())),
        (r#"ClassifyGroup("europe")"#, ExporterInfo::default(), ec(|c| c.group = "europe".into())),
        (r#"ClassifySite("paris")"#, ExporterInfo::default(), ec(|c| c.site = "paris".into())),
        (r#"ClassifyRole("edge")"#, ExporterInfo::default(), ec(|c| c.role = "edge".into())),
        (r#"ClassifyRegion("europe")"#, ExporterInfo::default(), ec(|c| c.region = "europe".into())),
        (r#"ClassifyTenant("mobile")"#, ExporterInfo::default(), ec(|c| c.tenant = "mobile".into())),
        (
            r#"ClassifyTenant(Format("tenant-%s", Exporter.Name))"#,
            exporter("127.0.0.1", "roger"),
            ec(|c| c.tenant = "tenant-roger".into()),
        ),
        (
            r#"Exporter.Name startsWith "expo" && Classify("europe")"#,
            exporter("127.0.0.1", "exporter"),
            ec(|c| c.group = "europe".into()),
        ),
        (
            r#"Exporter.Name matches "^e.p.r" && Classify("europe")"#,
            exporter("127.0.0.1", "exporter"),
            ec(|c| c.group = "europe".into()),
        ),
        (
            "Exporter.Name matches \"^e.p.r\" &&\nClassify(\"europe\")",
            exporter("127.0.0.1", "exporter"),
            ec(|c| c.group = "europe".into()),
        ),
        (
            r#"ClassifyRegex(Exporter.Name, "^(e.p+).r", "europe-$1")"#,
            exporter("127.0.0.1", "exporter"),
            ec(|c| c.group = "europe-exp".into()),
        ),
        (
            r#"ClassifyRegex(Exporter.Name, "^(\\w+).r", "europe-$1")"#,
            exporter("127.0.0.1", "exporter"),
            ec(|c| c.group = "europe-export".into()),
        ),
        (r#"ClassifyRegex(Exporter.Name, "^(ebp+).r", "europe-$1")"#, exporter("127.0.0.1", "exporter"), ec(|_| {})),
        (
            r#"ClassifyTenant("mobile") && Reject()"#,
            exporter("127.0.0.1", "exporter"),
            ec(|c| {
                c.tenant = "mobile".into();
                c.reject = true;
            }),
        ),
        (r#"Exporter.Name startsWith "nothing" && Reject()"#, exporter("127.0.0.1", "exporter"), ec(|_| {})),
        (
            r#"SetName(Format("%s-x", Exporter.Name))"#,
            exporter("127.0.0.1", "FRA_core1"),
            ec(|c| c.name = "FRA_core1-x".into()),
        ),
        (r#"SetName("first") && SetName("second")"#, ExporterInfo::default(), ec(|c| c.name = "first".into())),
        (
            r#"Exporter.Name matches "_RE\\d+$" && SetName(Exporter.Name[:lastIndexOf(Exporter.Name, "_RE")])"#,
            exporter("127.0.0.1", "FRA-core1_RE1"),
            ec(|c| c.name = "FRA-core1".into()),
        ),
        (
            r#"Exporter.Name matches "_RE\\d+$" && SetName(Exporter.Name[:lastIndexOf(Exporter.Name, "_RE")])"#,
            exporter("127.0.0.1", "FRA-core1"),
            ec(|_| {}),
        ),
        (r#"ClassifyRegex(Exporter.Name, "^(ebp+.r", "europe-$1")"#, exporter("127.0.0.1", "exporter"), None),
        (r#"Classify("europe""#, ExporterInfo::default(), None),
        ("Classify(1)", ExporterInfo::default(), None),
        (r#""hello""#, ExporterInfo::default(), None),
        (r#"ClassifyStuff("blip")"#, ExporterInfo::default(), None),
    ];
    for (program, info, expected) in cases {
        let rule = ExporterClassifierRule::new(program);
        let Some(expected) = expected else {
            if let Ok(rule) = rule {
                let mut c = ExporterClassification::default();
                assert!(rule.exec(&info, &mut c).is_err(), "{program}: expected an error");
            }
            continue;
        };
        let rule = rule.unwrap_or_else(|e| panic!("{program}: {e}"));
        let mut c = ExporterClassification::default();
        rule.exec(&info, &mut c).unwrap_or_else(|e| panic!("{program}: {e}"));
        assert_eq!(c, expected, "{program}");
    }
}

#[test]
fn interface_classifier() {
    let ii = |name: &str, description: &str, index: u32, vlan: u16| InterfaceInfo {
        index,
        name: name.into(),
        description: description.into(),
        speed: 0,
        vlan,
    };
    let ic = |f: fn(&mut InterfaceClassification)| {
        let mut c = InterfaceClassification::default();
        f(&mut c);
        Some(c)
    };
    let transit = "Transit: Telia (GWDM something something)";
    let cases: Vec<(&str, InterfaceInfo, Option<InterfaceClassification>)> = vec![
        ("false", InterfaceInfo::default(), ic(|_| {})),
        (r#"ClassifyConnectivity("Transit")"#, InterfaceInfo::default(), ic(|c| c.connectivity = "transit".into())),
        (r#"ClassifyProvider("Telia")"#, InterfaceInfo::default(), ic(|c| c.provider = "telia".into())),
        ("ClassifyExternal()", InterfaceInfo::default(), ic(|c| c.boundary = InterfaceBoundary::External)),
        ("ClassifyInternal()", InterfaceInfo::default(), ic(|c| c.boundary = InterfaceBoundary::Internal)),
        (
            r#"SetName("newname") && SetDescription("newdescription")"#,
            InterfaceInfo::default(),
            ic(|c| {
                c.name = "newname".into();
                c.description = "newdescription".into();
            }),
        ),
        (
            r#"ClassifyProvider(Format("II-%s", Interface.Name))"#,
            ii("Gi0/0/0", "", 0, 0),
            ic(|c| c.provider = "ii-gi000".into()),
        ),
        ("Reject()", InterfaceInfo::default(), ic(|c| c.reject = true)),
        ("Interface.Index == 200 && Reject()", ii("", "", 200, 0), ic(|c| c.reject = true)),
        (
            "\nInterface.Description startsWith \"Transit:\" &&\nClassifyConnectivity(\"transit\") &&\nClassifyExternal() &&\nClassifyProviderRegex(Interface.Description, \"^Transit: ([^ ]+)\", \"$1\")",
            ii("", transit, 0, 0),
            ic(|c| {
                c.connectivity = "transit".into();
                c.provider = "telia".into();
                c.boundary = InterfaceBoundary::External;
            }),
        ),
        (r#"ClassifyProviderRegex(Interface.Description, "^(ebp+.r", "europe-$1")"#, ii("", transit, 0, 0), None),
        (
            "Interface.VLAN == 100 && ClassifyExternal()",
            ii("", transit, 0, 100),
            ic(|c| c.boundary = InterfaceBoundary::External),
        ),
        ("Interface.VLAN == 100 && ClassifyExternal()", ii("", transit, 0, 200), ic(|_| {})),
    ];
    let exporter = exporter("127.0.0.1", "exporter");
    for (program, info, expected) in cases {
        let rule = InterfaceClassifierRule::new(program);
        let Some(expected) = expected else {
            if let Ok(rule) = rule {
                let mut c = InterfaceClassification::default();
                assert!(rule.exec(&exporter, &info, &mut c).is_err(), "{program}: expected an error");
            }
            continue;
        };
        let rule = rule.unwrap_or_else(|e| panic!("{program}: {e}"));
        let mut c = InterfaceClassification::default();
        rule.exec(&exporter, &info, &mut c).unwrap_or_else(|e| panic!("{program}: {e}"));
        assert_eq!(c, expected, "{program}");
    }
}

#[test]
fn documentation_example() {
    let rule = InterfaceClassifierRule::new(
        "ClassifyConnectivityRegex(Interface.Description, \"^(?i)(transit|pni|ppni|ix):? \", \"$1\") &&\nClassifyProviderRegex(Interface.Description, \"^\\\\S+?\\\\s(\\\\S+)\", \"$1\") &&\nClassifyExternal()\n",
    )
    .unwrap();
    let mut c = InterfaceClassification::default();
    let info = InterfaceInfo { description: "Transit: Cogent 1-3834938493".into(), ..Default::default() };
    rule.exec(&ExporterInfo::default(), &info, &mut c).unwrap();
    assert_eq!(
        (c.connectivity.as_str(), c.provider.as_str(), c.boundary),
        ("transit", "cogent", InterfaceBoundary::External)
    );
}
