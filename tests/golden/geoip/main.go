// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

// Command geoip dumps what the Go GeoIP component extracts from the test
// databases. The Rust port compares against it.
package main

import (
	"encoding/json"
	"fmt"
	"net/netip"
	"os"
	"path/filepath"
	"sort"

	"akvorado/common/daemon"
	"akvorado/common/reporter"
	"akvorado/outlet/geoip"
)

type entry struct {
	Prefix  string `json:"prefix"`
	ASN     uint32 `json:"asn,omitempty"`
	Country string `json:"country,omitempty"`
	State   string `json:"state,omitempty"`
	City    string `json:"city,omitempty"`
}

func main() {
	root := os.Args[1]
	files, _ := filepath.Glob(filepath.Join(root, "outlet/geoip/testdata/*.mmdb"))
	sort.Strings(files)
	out := map[string]map[string][]entry{}
	for _, file := range files {
		rel, _ := filepath.Rel(root, file)
		out[rel] = map[string][]entry{}
		for _, which := range []string{"asn", "geo"} {
			r, _ := reporter.New(reporter.DefaultConfiguration())
			d, _ := daemon.New(r)
			config := geoip.DefaultConfiguration()
			if which == "asn" {
				config.ASNDatabase = []string{file}
			} else {
				config.GeoDatabase = []string{file}
			}
			c, err := geoip.New(r, config, geoip.Dependencies{Daemon: d})
			if err != nil {
				panic(err)
			}
			if err := c.Start(); err != nil {
				panic(err)
			}
			entries := []entry{}
			if which == "asn" {
				c.IterASNDatabases(func(p netip.Prefix, i geoip.ASNInfo) {
					entries = append(entries, entry{Prefix: p.String(), ASN: i.ASNumber})
				})
			} else {
				c.IterGeoDatabases(func(p netip.Prefix, i geoip.GeoInfo) {
					entries = append(entries, entry{Prefix: p.String(), Country: i.Country, State: i.State, City: i.City})
				})
			}
			c.Stop()
			out[rel][which] = entries
		}
	}
	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", " ")
	if err := enc.Encode(out); err != nil {
		panic(err)
	}
	fmt.Fprintln(os.Stderr, "ok")
}
