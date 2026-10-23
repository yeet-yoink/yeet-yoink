# Changelog

All notable changes to this project will be documented in this file.
This project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Added

- The `/yeet` endpoint now supports the optional `?file_name=...` parameter for specifying
  the original file name as metadata to be returned with `/yoink`.
- Memcached backends accept a `max_file_size_bytes` setting. Files above the limit are
  not sent to that backend; it defaults to just under Memcached's 1 MiB item limit
  ([#59](https://github.com/yeet-yoink/yeet-yoink/issues/59)).
- Memcached backends accept a `priority` setting (default `0`). `/yoink` asks backends one at
  a time, highest priority first, and stops at the first intact copy; distributions start in
  the same order ([#56](https://github.com/yeet-yoink/yeet-yoink/issues/56)).
- Files can be distributed to and fetched from [Valkey](https://valkey.io/) via `backends.valkey`,
  with the same `expiration_sec`, `max_file_size_bytes` and `priority` settings as Memcached.
  Valkey backends connect on first use and reconnect on their own
  ([#12](https://github.com/yeet-yoink/yeet-yoink/issues/12)).
- Files can be distributed to and fetched from Google Cloud Storage via `backends.gcs`
  (bucket, optional prefix, Application Default Credentials or a service account key).
  Cloud Storage has no per-object expiry: the expiry is checked on fetch, and a bucket
  lifecycle rule should delete old objects
  ([#13](https://github.com/yeet-yoink/yeet-yoink/issues/13)).
- Files can be distributed to and fetched from Azure Blob Storage via `backends.azure`
  (account, container, optional prefix; an access key or SAS token file, or credentials from
  the environment such as AKS workload identity). As with Google Cloud Storage, the expiry is
  checked on fetch and a lifecycle management policy should delete old blobs. Both share one
  implementation, which stores file metadata as the custom attribute `yymetadata`.
- Instances find each other through a `cluster` section, within and across Kubernetes
  namespaces: seeds name headless services, gossip runs over UDP, encrypted and authenticated
  with a shared secret (AES-256-GCM, key derived with HKDF-SHA256). Settings can be overridden
  by `YY_`-prefixed environment variables, e.g. from the Downward API. See
  `deploy/kubernetes/cluster.yaml` ([#9](https://github.com/yeet-yoink/yeet-yoink/issues/9)).

### Changed

- The `backends.memcache` section is now optional, so Valkey-only configurations are valid.
- Files fetched from Valkey and Google Cloud Storage are streamed into the local buffer and
  checked against their hashes there, instead of being read into memory first. `/yoink`
  always serves the verified local copy; with `cache.near_cache: false` that copy is dropped
  once served ([#120](https://github.com/yeet-yoink/yeet-yoink/issues/120)).
- Files are distributed to Valkey and Google Cloud Storage in chunks instead of being read
  into memory first: Valkey appends to a temporary key that is renamed into place, and Cloud
  Storage uses multipart uploads for files above 8 MiB. Google Cloud Storage backends
  therefore no longer limit the file size by default
  ([#122](https://github.com/yeet-yoink/yeet-yoink/issues/122)).

### Fixed

- `/yeet` answers `400 Bad Request` instead of `500 Internal Server Error` when the request
  body does not match its `Content-MD5` or `Content-Length`, or is longer than announced.

## [0.0.1] - 2023-06-25

### Added

- The service can be used to store and retrieve files locally via the `/yeet` and `/yoink/:id` endpoints.
- Health check endpoints are available at `/startupz`, `/readyz`,
  `/livez`, as well as `/health` and `/healthz`.
- Prometheus/OpenMetrics metrics is available at `/metrics`.
- Shutdown from SIGINT (e.g. CTRL-C), SIGTERM and similar is now possible.
- The service now has a `/stop` endpoint to gracefully shut it down, freeing all open resources.
- The `transfer_size_total` and `transfer_total` metrics now track the number of bytes sent through the service.

### Internal

- 🎉 Initial release.

[0.0.1]: https://github.com/sunsided/yeet-yoink/releases/tag/0.0.1
