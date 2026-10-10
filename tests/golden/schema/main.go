// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

// Command schema dumps the output of the Go schema package for each case in
// testdata/schema-cases.yaml. The Rust port compares against it.
package main

import (
	"encoding/json"
	"fmt"
	"os"

	"github.com/go-viper/mapstructure/v2"

	"akvorado/common/helpers"
	"akvorado/common/helpers/yaml"
	"akvorado/common/schema"
)

type testCase struct {
	Name   string `yaml:"name"`
	Config any    `yaml:"config"`
}

type columnOut struct {
	Key                     uint   `json:"key"`
	Name                    string `json:"name"`
	Disabled                bool   `json:"disabled"`
	NoDisable               bool   `json:"no_disable"`
	Group                   uint   `json:"group"`
	Depends                 []uint `json:"depends"`
	ParserType              string `json:"parser_type"`
	ClickHouseType          string `json:"clickhouse_type"`
	ClickHouseCodec         string `json:"clickhouse_codec"`
	ClickHouseAlias         string `json:"clickhouse_alias"`
	ClickHouseNotSortingKey bool   `json:"clickhouse_not_sorting_key"`
	ClickHouseGenerateFrom  string `json:"clickhouse_generate_from"`
	ClickHouseMainOnly      bool   `json:"clickhouse_main_only"`
	ClickHouseMaterialized  bool   `json:"clickhouse_materialized"`
	ConsoleNotDimension     bool   `json:"console_not_dimension"`
	ConsoleTruncateIP       bool   `json:"console_truncate_ip"`
	ProtobufIndex           int    `json:"protobuf_index"`
	ProtobufType            string `json:"protobuf_type"`
	ProtobufRepeated        bool   `json:"protobuf_repeated"`
}

type output struct {
	Name               string              `json:"name"`
	Error              bool                `json:"error"`
	CreateTable        map[string]string   `json:"create_table,omitempty"`
	SelectColumns      map[string][]string `json:"select_columns,omitempty"`
	Hash               string              `json:"hash,omitempty"`
	SortingKeys        []string            `json:"sorting_keys,omitempty"`
	PrimaryKeys        []string            `json:"primary_keys,omitempty"`
	ProtobufDefinition string              `json:"protobuf_definition,omitempty"`
	ProtobufHash       string              `json:"protobuf_hash,omitempty"`
	Columns            []columnOut         `json:"columns,omitempty"`
	SkipIndexes        map[string]string   `json:"skip_indexes,omitempty"`
}

var options = map[string][]schema.ClickHouseTableOption{
	"none":                   nil,
	"skip_main_only":         {schema.ClickHouseSkipMainOnlyColumns},
	"skip_generated_aliased": {schema.ClickHouseSkipGeneratedColumns, schema.ClickHouseSkipAliasedColumns},
	"skip_time_main_alias":   {schema.ClickHouseSkipTimeReceived, schema.ClickHouseSkipMainOnlyColumns, schema.ClickHouseSkipAliasedColumns},
	"substitute_generates":   {schema.ClickHouseSubstituteGenerates, schema.ClickHouseSkipAliasedColumns},
}

func protobufKind(c schema.Column) string {
	if c.ProtobufType == 0 {
		return ""
	}
	return c.ProtobufType.String()
}

// lookup a column by key. end is true when the key is past the index.
func lookup(c *schema.Component, key schema.ColumnKey) (column *schema.Column, ok bool, end bool) {
	defer func() {
		if recover() != nil {
			end = true
		}
	}()
	column, ok = c.LookupColumnByKey(key)
	return column, ok, false
}

// newSchema builds a schema, turning panics into errors. Go panics when a
// custom dictionary column has a Src prefix and no Dst counterpart.
func newSchema(config schema.Configuration) (c *schema.Component, err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("panic: %v", r)
		}
	}()
	return schema.New(config)
}

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: schema <cases.yaml>")
		os.Exit(2)
	}
	raw, err := os.ReadFile(os.Args[1])
	if err != nil {
		panic(err)
	}
	var cases []testCase
	if err := yaml.Unmarshal(raw, &cases); err != nil {
		panic(err)
	}
	outputs := []output{}
	for _, tc := range cases {
		out := output{Name: tc.Name}
		config := schema.DefaultConfiguration()
		decoder, err := mapstructure.NewDecoder(helpers.GetMapStructureDecoderConfig(&config))
		if err != nil {
			panic(err)
		}
		if tc.Config != nil {
			err = decoder.Decode(tc.Config)
		}
		if err == nil {
			err = helpers.Validate.Struct(config)
		}
		var c *schema.Component
		if err == nil {
			c, err = newSchema(config)
		}
		if err != nil {
			out.Error = true
			outputs = append(outputs, out)
			continue
		}
		out.CreateTable = map[string]string{}
		out.SelectColumns = map[string][]string{}
		for name, opts := range options {
			out.CreateTable[name] = c.ClickHouseCreateTable(opts...)
			out.SelectColumns[name] = c.ClickHouseSelectColumns(opts...)
		}
		out.Hash = c.ClickHouseHash()
		out.SortingKeys = c.ClickHouseSortingKeys()
		out.PrimaryKeys = c.ClickHousePrimaryKeys()
		out.ProtobufDefinition = c.ProtobufDefinition()
		out.ProtobufHash = c.ProtobufMessageHash()
		out.SkipIndexes = map[string]string{}
		for k, v := range c.GetSkipIndexes() {
			t, _ := v.ClickHouseType()
			out.SkipIndexes[k.String()] = t
		}
		// All columns, including disabled ones, through the key index.
		for key := schema.ColumnTimeReceived; ; key++ {
			column, ok, end := lookup(c, key)
			if end {
				break
			}
			if !ok {
				continue
			}
			deps := []uint{}
			for _, d := range column.Depends {
				deps = append(deps, uint(d))
			}
			out.Columns = append(out.Columns, columnOut{
				Key:                     uint(column.Key),
				Name:                    column.Name,
				Disabled:                column.Disabled,
				NoDisable:               column.NoDisable,
				Group:                   uint(column.Group),
				Depends:                 deps,
				ParserType:              column.ParserType,
				ClickHouseType:          column.ClickHouseType,
				ClickHouseCodec:         column.ClickHouseCodec,
				ClickHouseAlias:         column.ClickHouseAlias,
				ClickHouseNotSortingKey: column.ClickHouseNotSortingKey,
				ClickHouseGenerateFrom:  column.ClickHouseGenerateFrom,
				ClickHouseMainOnly:      column.ClickHouseMainOnly,
				ClickHouseMaterialized:  column.ClickHouseMaterialized,
				ConsoleNotDimension:     column.ConsoleNotDimension,
				ConsoleTruncateIP:       column.ConsoleTruncateIP,
				ProtobufIndex:           int(column.ProtobufIndex),
				ProtobufType:            protobufKind(*column),
				ProtobufRepeated:        column.ProtobufRepeated,
			})
		}
		outputs = append(outputs, out)
	}
	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	if err := enc.Encode(outputs); err != nil {
		panic(err)
	}
}
