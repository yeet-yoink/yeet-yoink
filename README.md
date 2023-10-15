# yeet/yoink — A file storage and retrieval service

[![WIP](https://img.shields.io/badge/work_in_progress-yellow)](https://github.com/sunsided/yeet-yoink)
[![Latest version](https://img.shields.io/github/v/release/sunsided/yeet-yoink)](https://github.com/sunsided/yeet-yoink/releases)
[![License](https://img.shields.io/github/license/sunsided/yeet-yoink)](https://github.com/sunsided/yeet-yoink/blob/main/LICENSE.md)
[![Language](https://img.shields.io/github/languages/top/sunsided/yeet-yoink)](https://github.com/sunsided/yeet-yoink)
[![Build Status](https://github.com/sunsided/yeet-yoink/actions/workflows/rust.yml/badge.svg)](https://github.com/sunsided/yeet-yoink/actions/workflows/rust.yml)
[![Issues](https://img.shields.io/github/issues/sunsided/yeet-yoink)](https://github.com/sunsided/yeet-yoink/issues)
[![Dependency Status](https://img.shields.io/librariesio/github/sunsided/yeet-yoink)](https://libraries.io/github/sunsided/yeet-yoink)
[![Code size](https://img.shields.io/github/languages/code-size/sunsided/yeet-yoink)](https://github.com/sunsided/yeet-yoink)

A service to which you can yeet your files in order to yoink them from somewhere else.
This is meant to simplify cluster-local file sharing with configurable persistence backends.

One key aspect here is that a service like this can offload communication with object storage,
databases or other systems, allowing the actual storage and retrieval within the application
to be reduced to a simple HTTP POST and GET, given a file ID.

> ⚠️ **This service is work in progress.**

```mermaid
sequenceDiagram
    autonumber
    
    Alice->>Alice's yeyo: do yeet
    activate Alice;
    Note over Alice,Alice's yeyo: Alice stores a file
    activate Alice's yeyo;
    Alice's yeyo -) Storage Backend: take file;
    activate Storage Backend;
    deactivate Storage Backend;
    Alice's yeyo-->>Alice: okie #9829;
    deactivate Alice's yeyo;
    deactivate Alice;

    Bob->>Bob's yeyo: do yoink
    activate Bob;
    Note over Bob,Bob's yeyo: Bob needs the file
    activate Bob's yeyo;
    Bob's yeyo ->> Alice's yeyo: maybe yoink?
    activate Alice's yeyo;
    Note over Bob's yeyo,Alice's yeyo: Bob's yeyo attempts to fetch the file from the source
    
    alt has file
        Alice's yeyo --) Bob's yeyo: here file #9829;
        Note over Alice's yeyo,Bob's yeyo: If possible, Alice's yeyo returns the file directly
    else no file
        Alice's yeyo --) Bob's yeyo: sry no yoink
        Note over Alice's yeyo,Bob's yeyo: Eventually the file would be missing
        
        Bob's yeyo ->> Storage Backend: give file
        activate Storage Backend;
        Note over Bob's yeyo,Storage Backend: Bob's yeyo then talks directly to the Storage backend
        Storage Backend --) Bob's yeyo: here file
        deactivate Storage Backend;
    end
    deactivate Alice's yeyo;
    Bob's yeyo-->>Bob: here file
    deactivate Bob's yeyo;
    deactivate Bob;
```

## HTTP API

### Storing Files

* `/yeet` - Hands a file over to the service for storage and returns its ID.
  * `?file_name=...` - Optional. Allows to specify name metadata for the file.

### Retrieving files

* `/yoink/:id` - Retrieves a file from storage, given its ID.

### Metrics

* `/metrics` - Produces metrics in Prometheus/OpenMetrics format.

### Health Checks

* `/startupz` - Meant for Kubernetes startup probes. 
* `/readyz` - Meant for Kubernetes readiness probes. 
* `/livez` - Meant for Kubernetes liveness probes. 
* `/health` - Meant for complete health checks (e.g. by Google Cloud Load Balancer). 
* `/healthz` - Meant for human inspection.

### Shutdown

* `/stop` - Initiates a graceful shutdown.

## Example run

```shell
cargo run --bin yeet-yoink -- --http 127.0.0.1:8080 --http 127.0.1.1:8081 -c example-config.yaml
```
