// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

package filter

// Golden dump for the Rust port: run the filter parser on the inputs of the
// tests (and more) and write the results. Only runs when AKVORADO_GOLDEN_OUT
// is set:
//
//	AKVORADO_GOLDEN_OUT=$PWD/tests/golden/testdata/filter.json \
//	  go test ./console/filter -run TestGoldenDump -count=1

import (
	"encoding/json"
	"os"
	"regexp"
	"sort"
	"testing"

	"akvorado/common/schema"
	sb "akvorado/common/sqlbuilder"
)

type goldenParse struct {
	Input             string     `json:"input"`
	Schema            string     `json:"schema"`
	Reverse           bool       `json:"reverse"`
	Output            string     `json:"output,omitempty"`
	MainTableRequired bool       `json:"main_table_required"`
	Errors            []oneError `json:"errors,omitempty"`
	Human             string     `json:"human,omitempty"`
	Expected          []string   `json:"expected,omitempty"`
}

type goldenReverse struct {
	Input  string `json:"input"`
	Output string `json:"output"`
	Error  string `json:"error,omitempty"`
}

type goldenComplete struct {
	Column   string     `json:"column"`
	Expected []string   `json:"expected"`
	Errors   []oneError `json:"errors,omitempty"`
}

var goldenExtra = []string{
	``,
	`   `,
	`SrcAS = 1 AND DstAS = 2 OR SrcPort = 3`,
	`SrcAS = 1 OR DstAS = 2 AND SrcPort = 3`,
	`NOT SrcAS = 1 AND NOT (DstAS = 2 OR SrcPort = 3)`,
	`NOT NOT SrcAS = 1`,
	`(SrcAS = 1)`,
	`((SrcAS = 1 OR DstAS = 2)) AND (SrcPort = 3)`,
	`SrcAS = 1 -- comment`,
	`SrcAS = 1 /* comment */ AND /* other */ DstAS = 2`,
	"SrcAS = 1\nAND\nDstAS = 2",
	"SrcAS = 1\nAND\nDstAS = ",
	`DstCommunities = 65000:100`,
	`DstCommunities != 65000:100`,
	`DstCommunities = 65000:100:200`,
	`DstCommunities != 4200000000:1:2`,
	`DstCommunities = 65536:100`,
	`DstASPath = AS174`,
	`DstASPath != 174`,
	`SrcMAC = 00:11:22:33:44:55`,
	`SrcMAC IN (00:11:22:33:44:55, aa:bb:cc:dd:ee:ff)`,
	`SrcMAC != 0011.2233.4455`,
	`SrcAddr << 192.0.2.0/24`,
	`SrcAddr !<< 192.0.2.0/24`,
	`SrcAddr != 192.0.2.0/24`,
	`SrcAddr IN (192.0.2.0/24, 2001:db8::1, 198.51.100.1)`,
	`SrcAddr NOTIN (192.0.2.0/24)`,
	`SrcAddr IN (192.0.2.1)`,
	`SrcNetPrefix = 192.168.0.128/27`,
	`DstNetPrefix != 2001:db8::/48`,
	`Proto = TCP`,
	`Proto = "TCP"`,
	`Proto IN ("TCP", "UDP")`,
	`Proto >= 6`,
	`Proto = 256`,
	`EType = ipv4`,
	`EType != IPv6`,
	`InIfBoundary = External`,
	`FlowDirection = ingress`,
	`FlowDirection != EGRESS`,
	`SrcPort IN (80, 443)`,
	`SrcPort NOTIN (80)`,
	`SrcPort > DstPort`,
	`ForwardingStatus >= 128`,
	`ExporterName ILIKE "%a%"`,
	`ExporterName IUNLIKE "%a%"`,
	`ExporterName UNLIKE "%a%"`,
	`ExporterName = ExporterGroup`,
	`InIfSpeed = 18446744073709551615`,
	`InIfSpeed = 18446744073709551616`,
	`SrcAS = AS4294967295`,
	`SrcAS = 4294967296`,
	`ExporterAddress = 192.0.2.1 AND SrcNetPrefix = 192.0.2.0/24 OR DstMAC = 00:00:00:00:00:01`,
	`ExporterName = "unterminated`,
	"ExporterName = 'unterminated\nAND SrcAS = 1",
	`SrcAS = 1 /* unclosed`,
	`SrcAS`,
	`= 1`,
	`SrcAS = 1 AND`,
	`SrcAS = 1 OR OR`,
	`(SrcAS = 1`,
	`SrcAS = 1)`,
	`SrcAddr = 300.1.1.1`,
	`SrcAddr << 192.0.2.0/33`,
	`SrcMAC = 00:11:22:33:44`,
	`DstCommunities = 1:2:3:4`,
	`Unknown = 1`,
	`SrcAS = 1 AND Unknown = 2`,
	`Dst1stAS = 174`,
	`SrcGeoCity = "Paris"`,
	`MPLSLabels = 100`,
	`SrcVlan = 10`,
	`IPTTL > 3`,
	`InIfName = "a" AND OutIfName = "b" AND SrcNetName = "c" AND DstNetRole = "d"`,
}

func goldenSchemas(t *testing.T) map[string]*schema.Component {
	all := schema.NewMock(t).EnableAllColumns()
	config := schema.DefaultConfiguration()
	config.CustomDictionaries = map[string]schema.CustomDict{
		"test": {
			Keys:       []schema.CustomDictKey{{Name: "SrcAddr", Type: "string"}},
			Attributes: []schema.CustomDictAttribute{{Name: "csv_col_name", Type: "string", Label: "DimensionAttribute"}, {Name: "role", Type: "string"}},
			Source:     "test.csv",
			Dimensions: []string{"SrcAddr", "DstAddr"},
		},
	}
	custom, err := schema.New(config)
	if err != nil {
		t.Fatal(err)
	}
	return map[string]*schema.Component{
		"default": schema.NewMock(t),
		"all":     all,
		"custom":  custom,
	}
}

func TestGoldenDump(t *testing.T) {
	out := os.Getenv("AKVORADO_GOLDEN_OUT")
	if out == "" {
		t.Skip("AKVORADO_GOLDEN_OUT not set")
	}
	// Inputs of the tests.
	seen := map[string]bool{}
	inputs := []string{}
	re := regexp.MustCompile("Input: *`([^`]*)`")
	for _, file := range []string{"parser_test.go", "reverse_test.go", "error_test.go", "../filter_test.go", "../query/filter_test.go"} {
		data, err := os.ReadFile(file)
		if err != nil {
			t.Fatal(err)
		}
		for _, m := range re.FindAllStringSubmatch(string(data), -1) {
			if !seen[m[1]] {
				seen[m[1]] = true
				inputs = append(inputs, m[1])
			}
		}
	}
	for _, in := range goldenExtra {
		if !seen[in] {
			seen[in] = true
			inputs = append(inputs, in)
		}
	}

	schemas := goldenSchemas(t)
	names := []string{"default", "all", "custom"}
	parses := []goldenParse{}
	for _, input := range inputs {
		for _, name := range names {
			for _, reverse := range []bool{false, true} {
				meta := &Meta{Schema: schemas[name], Database: "default", ReverseDirection: reverse}
				got, err := Parse("", []byte(input), GlobalStore("meta", meta))
				g := goldenParse{Input: input, Schema: name, Reverse: reverse, MainTableRequired: meta.MainTableRequired}
				if err != nil {
					g.Errors = AllErrors(err)
					g.Human = HumanError(err)
					g.Expected = Expected(err)
				} else if expr, ok := got.(sb.Expr); ok {
					g.Output = expr.String()
				}
				parses = append(parses, g)
			}
		}
	}

	reverses := []goldenReverse{}
	for _, input := range inputs {
		r, err := Reverse(input, schemas["all"])
		g := goldenReverse{Input: input, Output: r}
		if err != nil {
			g.Error = err.Error()
		}
		reverses = append(reverses, g)
	}

	completes := map[string][]goldenComplete{}
	for _, name := range names {
		columns := []string{}
		for _, column := range schemas[name].Columns() {
			columns = append(columns, column.Name)
		}
		columns = append(columns, "Unknown", "srcas")
		sort.Strings(columns)
		for _, column := range columns {
			_, err := Parse("", []byte(column+" "), Entrypoint("ConditionExpr"),
				GlobalStore("meta", &Meta{Schema: schemas[name], Database: "default"}))
			completes[name] = append(completes[name], goldenComplete{
				Column:   column,
				Expected: Expected(err),
				Errors:   AllErrors(err),
			})
		}
	}

	data, err := json.MarshalIndent(map[string]any{
		"parse":    parses,
		"reverse":  reverses,
		"complete": completes,
	}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(out, data, 0o644); err != nil {
		t.Fatal(err)
	}
	t.Logf("%d inputs", len(inputs))
}
