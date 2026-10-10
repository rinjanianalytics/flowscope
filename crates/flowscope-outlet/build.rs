// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Generate the BioRIS and gNMI gRPC clients (and the servers, used by
//! tests) from the vendored protobuf files. protox compiles them, so protoc is not
//! needed.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = ["cmd/ris/api/ris.proto", "route/api/route.proto", "net/api/net.proto"];
    for f in files {
        println!("cargo:rerun-if-changed=proto/bio/{f}");
    }
    let fds = protox::compile(files, ["proto/bio"])?;
    tonic_prost_build::configure().build_client(true).build_server(true).compile_fds(fds)?;

    // gNMI, for the metadata provider.
    let files = ["github.com/openconfig/gnmi/proto/gnmi/gnmi.proto"];
    println!("cargo:rerun-if-changed=proto/gnmi");
    let fds = protox::compile(files, ["proto/gnmi"])?;
    tonic_prost_build::configure().build_client(true).build_server(true).compile_fds(fds)?;
    Ok(())
}
