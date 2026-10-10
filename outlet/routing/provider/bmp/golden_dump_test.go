// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

package bmp

// Golden dump for the Rust port: replay BMP captures through the provider
// and write the RIB and the metrics after each step. Only runs when
// AKVORADO_GOLDEN_OUT is set:
//
//	AKVORADO_GOLDEN_OUT=$PWD/tests/golden/testdata/bmp.json \
//	  go test ./outlet/routing/provider/bmp -run TestGoldenDump -count=1

import (
	"encoding/json"
	"fmt"
	"net"
	"net/netip"
	"os"
	"path"
	"slices"
	"strings"
	"testing"
	"time"

	"akvorado/common/helpers"
	"akvorado/common/reporter"
)

type goldenScenario struct {
	Name   string   `json:"name"`
	RDs    []string `json:"rds,omitempty"`
	RTs    []string `json:"rts,omitempty"`
	NoASNs bool     `json:"no_asns,omitempty"`
	NoPath bool     `json:"no_as_paths,omitempty"`
	NoComm bool     `json:"no_communities,omitempty"`
	// Steps: "dial", "send <conn> <pcap>", "close <conn>", "clock <duration>", "snapshot"
	Steps []string `json:"steps"`
}

type goldenSnapshot struct {
	RIB     map[string][]string `json:"rib"`
	Metrics map[string]string   `json:"metrics"`
}

type goldenResult struct {
	goldenScenario
	Snapshots []goldenSnapshot `json:"snapshots"`
}

func goldenScenarios() []goldenScenario {
	std := []string{"dial", "send 0 bmp-init.pcap", "send 0 bmp-peers-up.pcap", "send 0 bmp-eor.pcap", "send 0 bmp-reach.pcap"}
	l3vpn := []string{"dial", "send 0 bmp-init.pcap", "send 0 bmp-l3vpn.pcap"}
	with := func(base []string, more ...string) []string {
		return append(slices.Clone(base), more...)
	}
	return []goldenScenario{
		{Name: "init, terminate", Steps: []string{"dial", "send 0 bmp-init.pcap", "snapshot", "send 0 bmp-terminate.pcap", "snapshot", "clock 2h"}},
		{Name: "init, peers up, eor", Steps: []string{"dial", "send 0 bmp-init.pcap", "send 0 bmp-peers-up.pcap", "send 0 bmp-eor.pcap"}},
		{Name: "init, peers up, eor, reach NLRI", Steps: with(std, "send 0 bmp-reach-addpath.pcap")},
		{Name: "init, no peers up, reach NLRI", Steps: []string{"dial", "send 0 bmp-init.pcap", "send 0 bmp-reach.pcap"}},
		{Name: "init, no peers up, reach NLRI, peers up", Steps: []string{"dial", "send 0 bmp-init.pcap", "send 0 bmp-reach.pcap", "send 0 bmp-peers-up.pcap", "send 0 bmp-eor.pcap"}},
		{Name: "1 peer down", Steps: with(std, "send 0 bmp-peer-down.pcap")},
		{Name: "RD 65017:104", RDs: []string{"65017:104"}, Steps: std},
		{Name: "RD 0:0", RDs: []string{"0:0"}, Steps: std},
		{Name: "RT 65017:101", RTs: []string{"65017:101"}, Steps: std},
		{Name: "RT 65017:103", RTs: []string{"65017:103"}, Steps: std},
		{Name: "RT 1:1", RTs: []string{"1:1"}, Steps: std},
		{Name: "RD 65017:104 and RT 65017:103", RDs: []string{"65017:104"}, RTs: []string{"65017:103"}, Steps: std},
		{Name: "RT 0", RTs: []string{"0"}, Steps: std},
		{Name: "reach, unreach", RDs: []string{"0:0"}, Steps: with(std, "send 0 bmp-unreach.pcap")},
		{Name: "l3vpn", Steps: l3vpn},
		{Name: "loc-rib", Steps: []string{"dial", "send 0 bmp-locrib.pcap"}},
		{Name: "l3vpn RD 65500:108", RDs: []string{"65500:108"}, Steps: l3vpn},
		{Name: "l3vpn RD 65500:110", RDs: []string{"65500:110"}, Steps: l3vpn},
		{Name: "l3vpn no AS paths or communities", NoPath: true, NoComm: true, Steps: l3vpn},
		{Name: "l3vpn no ASNs", NoASNs: true, NoComm: true, Steps: l3vpn},
		{Name: "unreach only", Steps: []string{"dial", "send 0 bmp-init.pcap", "send 0 bmp-peers-up.pcap", "send 0 bmp-eor.pcap", "send 0 bmp-unreach.pcap"}},
		{Name: "reach, unreach×2", NoASNs: true, NoPath: true, NoComm: true, Steps: with(std, "send 0 bmp-unreach.pcap", "send 0 bmp-unreach.pcap")},
		{Name: "reach×2, unreach", Steps: with(std, "send 0 bmp-reach.pcap", "send 0 bmp-unreach.pcap")},
		{Name: "reach before eor", Steps: []string{"dial", "send 0 bmp-init.pcap", "send 0 bmp-peers-up.pcap", "send 0 bmp-reach.pcap", "send 0 bmp-eor.pcap"}},
		{Name: "l3vpn connection down", NoPath: true, NoComm: true, Steps: with(l3vpn, "close 0", "snapshot", "clock 4m", "snapshot", "clock 2m")},
		{Name: "l3vpn unknown family", Steps: with(l3vpn, "send 0 bmp-reach-unknown-family.pcap")},
		{Name: "l3vpn vpls", Steps: with(l3vpn, "send 0 bmp-reach-vpls.pcap")},
		{Name: "two connections", NoPath: true, NoComm: true, Steps: []string{
			"dial", "send 0 bmp-init.pcap", "send 0 bmp-l3vpn.pcap",
			"dial", "send 1 bmp-init.pcap", "send 1 bmp-l3vpn.pcap",
			"close 0", "snapshot", "clock 6m", "snapshot",
			"send 1 bmp-terminate.pcap", "snapshot", "clock 6m",
		}},
		{Name: "all captures", Steps: []string{
			"dial", "send 0 bmp-init.pcap", "send 0 bmp-peers-up.pcap", "send 0 bmp-eor.pcap",
			"send 0 bmp-reach.pcap", "send 0 bmp-reach-addpath.pcap", "send 0 bmp-l3vpn.pcap",
			"send 0 bmp-reach-unknown-family.pcap", "send 0 bmp-reach-vpls.pcap", "send 0 bmp-unreach.pcap",
			"send 0 bmp-peer-down.pcap",
		}},
	}
}

func goldenDumpRIB(p *Provider) map[string][]string {
	p.mu.RLock()
	defer p.mu.RUnlock()
	result := map[string][]string{}
	for prefix, ref := range p.rib.tree.Load().All() {
		rs := p.rib.shards[ref.idx.shardIdx()]
		rs.mu.RLock()
		for route := range rs.iterateRoutesForPrefixIndex(ref.idx) {
			nlriRef := rs.nlris.Get(route.nlri)
			nh := rs.nextHops.Get(route.nextHop)
			attrs := rs.rtas.Get(route.attributes)
			var peer netip.Addr
			for pkey, pinfo := range p.peers {
				if pinfo.reference == route.peer {
					peer = pkey.ip
					break
				}
			}
			key := peer.Unmap().String()
			result[key] = append(result[key],
				fmt.Sprintf("[%s] %s via %s %s/%d %d %v %v %v plen=%d",
					nlriRef.family,
					helpers.UnmapPrefix(prefix), netip.Addr(nh).Unmap(),
					nlriRef.rd, nlriRef.path,
					attrs.asn, attrs.asPath, attrs.communities, attrs.largeCommunities,
					route.prefixLen))
		}
		rs.mu.RUnlock()
	}
	for k := range result {
		slices.Sort(result[k])
	}
	return result
}

func TestGoldenDump(t *testing.T) {
	out := os.Getenv("AKVORADO_GOLDEN_OUT")
	if out == "" {
		t.Skip("AKVORADO_GOLDEN_OUT not set")
	}
	results := []goldenResult{}
	for _, sc := range goldenScenarios() {
		r := reporter.NewMock(t)
		config := DefaultConfiguration().(Configuration)
		for _, rd := range sc.RDs {
			config.RDs = append(config.RDs, MustParseRD(rd))
		}
		for _, rt := range sc.RTs {
			config.RTs = append(config.RTs, MustParseRT(rt))
		}
		config.CollectASNs = !sc.NoASNs
		config.CollectASPaths = !sc.NoPath
		config.CollectCommunities = !sc.NoComm
		p, mockClock := NewMock(t, r, config)
		helpers.StartStop(t, p)
		conns := []net.Conn{}
		res := goldenResult{goldenScenario: sc}
		snapshot := func() {
			time.Sleep(50 * time.Millisecond)
			res.Snapshots = append(res.Snapshots, goldenSnapshot{
				RIB:     goldenDumpRIB(p),
				Metrics: r.GetMetrics("akvorado_outlet_routing_provider_bmp_", "-locked_duration", "-buffer_size", "-message_queue"),
			})
		}
		for _, step := range sc.Steps {
			f := strings.Fields(step)
			switch f[0] {
			case "dial":
				conn, err := net.Dial("tcp", p.LocalAddr().String())
				if err != nil {
					t.Fatalf("Dial() error:\n%+v", err)
				}
				conns = append(conns, conn)
				time.Sleep(10 * time.Millisecond)
			case "send":
				var i int
				fmt.Sscan(f[1], &i)
				if _, err := conns[i].Write(helpers.ReadPcapL4(t, path.Join("testdata", f[2]))); err != nil {
					t.Fatalf("Write() error:\n%+v", err)
				}
				time.Sleep(30 * time.Millisecond)
			case "close":
				var i int
				fmt.Sscan(f[1], &i)
				conns[i].Close()
				time.Sleep(30 * time.Millisecond)
			case "clock":
				d, _ := time.ParseDuration(f[1])
				mockClock.Add(d)
				time.Sleep(30 * time.Millisecond)
			case "snapshot":
				snapshot()
			}
		}
		snapshot()
		for _, c := range conns {
			c.Close()
		}
		results = append(results, res)
	}
	data, err := json.MarshalIndent(results, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(out, data, 0o644); err != nil {
		t.Fatal(err)
	}
}
