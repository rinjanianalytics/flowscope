// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

// Command decoders runs the Go flow decoders over the pcap files used by the
// Go tests and dumps the decoded flows as JSON. The Rust decoders compare
// against it.
package main

import (
	"encoding/json"
	"fmt"
	"net/netip"
	"os"
	"path/filepath"
	"sort"
	"time"

	"github.com/google/gopacket"
	"github.com/google/gopacket/layers"
	"github.com/google/gopacket/pcapgo"

	"akvorado/common/pb"
	"akvorado/common/reporter"
	"akvorado/common/schema"
	"akvorado/outlet/flow/decoder"
	"akvorado/outlet/flow/decoder/netflow"
	"akvorado/outlet/flow/decoder/sflow"
)

// Same timestamp for every packet.
const timeReceived = 1_700_000_000

type flowOut struct {
	TimeReceived    uint32         `json:"time_received"`
	SamplingRate    uint64         `json:"sampling_rate"`
	ExporterAddress string         `json:"exporter_address"`
	InIf            uint32         `json:"in_if"`
	OutIf           uint32         `json:"out_if"`
	SrcVlan         uint16         `json:"src_vlan"`
	DstVlan         uint16         `json:"dst_vlan"`
	SrcAddr         string         `json:"src_addr"`
	DstAddr         string         `json:"dst_addr"`
	NextHop         string         `json:"next_hop"`
	SrcAS           uint32         `json:"src_as"`
	DstAS           uint32         `json:"dst_as"`
	SrcNetMask      uint8          `json:"src_net_mask"`
	DstNetMask      uint8          `json:"dst_net_mask"`
	Columns         map[string]any `json:"columns"`
}

type packetOut struct {
	Error bool      `json:"error"`
	Count int       `json:"count"`
	Flows []flowOut `json:"flows"`
}

type caseOut struct {
	Name            string      `json:"name"`
	Decoder         string      `json:"decoder"`
	File            string      `json:"file"`
	AllColumns      bool        `json:"all_columns"`
	TimestampSource string      `json:"timestamp_source"`
	Decapsulation   string      `json:"decapsulation"`
	Packets         []packetOut `json:"packets"`
}

type l2Out struct {
	File          string  `json:"file"`
	AllColumns    bool    `json:"all_columns"`
	Decapsulation string  `json:"decapsulation"`
	L3Length      uint64  `json:"l3_length"`
	Flow          flowOut `json:"flow"`
}

type output struct {
	Decoders []caseOut                  `json:"decoders"`
	Ethernet []l2Out                    `json:"ethernet"`
	States   map[string]json.RawMessage `json:"netflow_states"`
}

func addr(a netip.Addr) string {
	if !a.IsValid() {
		return ""
	}
	return a.String()
}

func value(v any) any {
	switch v := v.(type) {
	case netip.Addr:
		return addr(v)
	case []schema.UInt128:
		out := []string{}
		for _, x := range v {
			out = append(out, fmt.Sprintf("%d:%d", x.High, x.Low))
		}
		return out
	default:
		return v
	}
}

func capture(bf *schema.FlowMessage) flowOut {
	columns := map[string]any{}
	for k, v := range bf.OtherColumns {
		columns[k.String()] = value(v)
	}
	return flowOut{
		TimeReceived:    bf.TimeReceived,
		SamplingRate:    bf.SamplingRate,
		ExporterAddress: addr(bf.ExporterAddress),
		InIf:            bf.InIf,
		OutIf:           bf.OutIf,
		SrcVlan:         bf.SrcVlan,
		DstVlan:         bf.DstVlan,
		SrcAddr:         addr(bf.SrcAddr),
		DstAddr:         addr(bf.DstAddr),
		NextHop:         addr(bf.NextHop),
		SrcAS:           bf.SrcAS,
		DstAS:           bf.DstAS,
		SrcNetMask:      bf.SrcNetMask,
		DstNetMask:      bf.DstNetMask,
		Columns:         columns,
	}
}

func packets(file string, l4 bool) [][]byte {
	f, err := os.Open(file)
	if err != nil {
		panic(err)
	}
	defer f.Close()
	reader, err := pcapgo.NewReader(f)
	if err != nil {
		panic(err)
	}
	source := gopacket.NewPacketSource(reader, layers.LayerTypeEthernet)
	out := [][]byte{}
	for packet := range source.Packets() {
		if l4 {
			out = append(out, append([]byte{}, packet.TransportLayer().LayerPayload()...))
		} else {
			data := append([]byte{}, packet.LinkLayer().LayerContents()...)
			out = append(out, append(data, packet.LinkLayer().LayerPayload()...))
		}
	}
	return out
}

func newSchema(all bool) *schema.Component {
	sch, err := schema.New(schema.DefaultConfiguration())
	if err != nil {
		panic(err)
	}
	if all {
		sch = sch.EnableAllColumns()
	}
	return sch
}

var (
	timestampSources = map[string]pb.RawFlow_TimestampSource{
		"input":                  pb.RawFlow_TS_INPUT,
		"netflow-packet":         pb.RawFlow_TS_NETFLOW_PACKET,
		"netflow-first-switched": pb.RawFlow_TS_NETFLOW_FIRST_SWITCHED,
	}
	decapsulations = map[string]pb.RawFlow_DecapsulationProtocol{
		"none":  pb.RawFlow_DECAP_NONE,
		"ipip":  pb.RawFlow_DECAP_IPIP,
		"gre":   pb.RawFlow_DECAP_GRE,
		"vxlan": pb.RawFlow_DECAP_VXLAN,
		"srv6":  pb.RawFlow_DECAP_SRV6,
	}
)

func sortedKeys[V any](m map[string]V) []string {
	keys := []string{}
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: decoders <repository root>")
		os.Exit(2)
	}
	root := os.Args[1]
	out := output{Decoders: []caseOut{}, Ethernet: []l2Out{}, States: map[string]json.RawMessage{}}
	source := netip.MustParseAddr("::ffff:127.0.0.1")

	for _, dec := range []string{"netflow", "sflow"} {
		files, _ := filepath.Glob(filepath.Join(root, "outlet/flow/decoder", dec, "testdata", "*.pcap"))
		sort.Strings(files)
		for _, file := range files {
			pkts := packets(file, true)
			for _, all := range []bool{true, false} {
				for _, tsName := range sortedKeys(timestampSources) {
					if dec == "sflow" && tsName != "input" {
						continue
					}
					for _, decapName := range sortedKeys(decapsulations) {
						r, err := reporter.New(reporter.DefaultConfiguration())
						if err != nil {
							panic(err)
						}
						sch := newSchema(all)
						var d decoder.Decoder
						if dec == "netflow" {
							d = netflow.New(r, decoder.Dependencies{Schema: sch})
						} else {
							d = sflow.New(r, decoder.Dependencies{Schema: sch})
						}
						bf := sch.NewFlowMessage()
						rel, _ := filepath.Rel(root, file)
						c := caseOut{
							Name:            fmt.Sprintf("%s/%s/all=%v/ts=%s/decap=%s", dec, filepath.Base(file), all, tsName, decapName),
							Decoder:         dec,
							File:            rel,
							AllColumns:      all,
							TimestampSource: tsName,
							Decapsulation:   decapName,
						}
						for _, payload := range pkts {
							flows := []flowOut{}
							finalize := func() {
								flows = append(flows, capture(bf))
								bf.Clear()
							}
							n, err := d.Decode(decoder.RawFlow{
								TimeReceived: time.Unix(timeReceived, 0),
								Payload:      payload,
								Source:       source,
							}, decoder.Options{
								TimestampSource:       timestampSources[tsName],
								DecapsulationProtocol: decapsulations[decapName],
							}, bf, finalize)
							c.Packets = append(c.Packets, packetOut{Error: err != nil, Count: n, Flows: flows})
						}
						out.Decoders = append(out.Decoders, c)
						if dec == "netflow" && all && tsName == "input" && decapName == "none" {
							state, err := json.Marshal(d)
							if err != nil {
								panic(err)
							}
							out.States[rel] = state
						}
					}
				}
			}
		}
	}

	files, _ := filepath.Glob(filepath.Join(root, "outlet/flow/decoder/testdata", "*.pcap"))
	sort.Strings(files)
	for _, file := range files {
		pkts := packets(file, false)
		if len(pkts) != 1 {
			panic(fmt.Sprintf("%s: expected one packet", file))
		}
		for _, all := range []bool{true, false} {
			for _, decapName := range sortedKeys(decapsulations) {
				sch := newSchema(all)
				bf := sch.NewFlowMessage()
				l3 := decoder.ParseEthernet(sch, bf, decapsulations[decapName], pkts[0])
				rel, _ := filepath.Rel(root, file)
				out.Ethernet = append(out.Ethernet, l2Out{
					File:          rel,
					AllColumns:    all,
					Decapsulation: decapName,
					L3Length:      l3,
					Flow:          capture(bf),
				})
			}
		}
	}

	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", " ")
	if err := enc.Encode(out); err != nil {
		panic(err)
	}
}
