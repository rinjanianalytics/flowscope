// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

// Command bioris starts a real bio-rd RIS server fed with the BMP captures of
// the Go tests, for the Rust BioRIS provider interop test. It prints the gRPC
// address, then serves until stdin is closed.
//
//	go run ./tests/golden/bioris
package main

import (
	"encoding/binary"
	"fmt"
	"io"
	"net"
	"os"
	"path"
	"time"

	pb "github.com/bio-routing/bio-rd/cmd/ris/api"
	"github.com/bio-routing/bio-rd/cmd/ris/risserver"
	"github.com/bio-routing/bio-rd/protocols/bgp/server"
	"google.golang.org/grpc"
)

// tcpPayload returns the TCP payloads of a pcap file (Ethernet, IPv4 or
// IPv6), concatenated.
func tcpPayload(file string) []byte {
	data, err := os.ReadFile(file)
	if err != nil {
		panic(err)
	}
	var order binary.ByteOrder = binary.LittleEndian
	if binary.LittleEndian.Uint32(data) != 0xa1b2c3d4 {
		order = binary.BigEndian
	}
	out := []byte{}
	for o := 24; o+16 <= len(data); {
		caplen := int(order.Uint32(data[o+8:]))
		p := data[o+16+14 : o+16+caplen]
		o += 16 + caplen
		if p[0]>>4 == 4 {
			ihl := int(p[0]&0xf) * 4
			p = p[ihl:binary.BigEndian.Uint16(p[2:])]
		} else {
			p = p[40 : 40+binary.BigEndian.Uint16(p[4:])]
		}
		out = append(out, p[int(p[12]>>4)*4:]...)
	}
	return out
}

func main() {
	b := server.NewBMPReceiver(server.BMPReceiverConfig{KeepalivePeriod: 10 * time.Second, AcceptAny: true})
	if err := b.Listen("127.0.0.1:0"); err != nil {
		panic(err)
	}
	go b.Serve()
	conn, err := net.Dial("tcp", b.LocalAddr().String())
	if err != nil {
		panic(err)
	}
	for _, pcap := range []string{"bmp-init.pcap", "bmp-peers-up.pcap", "bmp-eor.pcap", "bmp-reach.pcap"} {
		if _, err := conn.Write(tcpPayload(path.Join("outlet/routing/provider/bmp/testdata", pcap))); err != nil {
			panic(err)
		}
	}
	time.Sleep(200 * time.Millisecond)

	rpc := grpc.NewServer()
	pb.RegisterRoutingInformationServiceServer(rpc, risserver.NewServer(b))
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		panic(err)
	}
	go rpc.Serve(l)
	fmt.Println(l.Addr().String())
	io.Copy(io.Discard, os.Stdin)
}
