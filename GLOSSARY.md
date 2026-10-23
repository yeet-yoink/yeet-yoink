# yeet/yoink

A cluster-local file sharing service: applications hand files to it and fetch them back by ID,
while it takes care of keeping copies in configurable storage backends.

## Language

### Files and requests

**Yeet**:
Handing a file to the service for storage, which returns the file's ID.
_Avoid_: Upload, put, store request

**Yoink**:
Retrieving a previously yeeted file by its ID.
_Avoid_: Download, get

**File ID**:
The short, random identifier the service assigns to a yeeted file; the only handle clients need to yoink it.
_Avoid_: Key, GUID, file name

**File metadata**:
The file name, content type and content hashes that travel with a file and are returned on yoink.
_Avoid_: Headers, attributes

**Yeyo**:
A single running instance of the service.
_Avoid_: Node, server, pod

**Peer**:
Another yeyo in the same cluster that may hold a file locally.
_Avoid_: Neighbor, replica

**Cluster**:
The yeyos that gossip with each other because they share a cluster ID and secret, possibly across Kubernetes namespaces.
_Avoid_: Mesh, ring, group

**Seed**:
A `host:port` a yeyo contacts first to join the cluster; usually the headless service of a namespace, which stands for all of its pods.
_Avoid_: Bootstrap node, contact point

**Gossip**:
The periodic exchange of state between yeyos through which every member learns of all others.
_Avoid_: Heartbeat, broadcast

### Local buffering

**Buffered file**:
A file held locally by a yeyo, either because it was yeeted there or because it was fetched from a storage backend.
_Avoid_: Temp file, local copy

**Lease**:
The time span during which a buffered file stays available to readers before the yeyo discards it.
_Avoid_: TTL, retention

**Near cache**:
Buffering a file fetched from a storage backend so that repeated yoinks are served locally.
_Avoid_: Read-through cache, local cache

### Storage backends

**Storage backend**:
An external system, such as Memcached, Valkey, Google Cloud Storage or Azure Blob Storage, that keeps copies of files beyond a yeyo's lease.
_Avoid_: Store, provider, persistence layer

**Backend tag**:
The configured name that identifies one storage backend instance.
_Avoid_: Backend ID, backend name

**Distribution**:
Sending a yeeted file to the storage backends after it has been buffered.
_Avoid_: Replication, sync, upload

**Fetch**:
Retrieving a file from a storage backend when no yeyo holds it locally.
_Avoid_: Download, pull

**Backend copy**:
The copy of a file a storage backend holds, together with its file metadata.
_Avoid_: Replica, backup

**Expiry**:
The point in time after which a backend copy must no longer be served or cached; a backend copy without expiry is kept indefinitely.
_Avoid_: TTL, expiration date

**Intact copy**:
A backend copy whose content matches the hashes in its file metadata. A copy that does not match is **corrupt** and is never served.
_Avoid_: Valid copy, verified copy

**Backend priority**:
The rank that decides which storage backend a fetch asks first; lower-ranked backends are only asked when higher-ranked ones have no intact copy.
_Avoid_: Weight, preference, tier

**Maximum file size**:
The largest file a storage backend accepts for distribution; larger files are not sent to it.
_Avoid_: Size cap, quota
