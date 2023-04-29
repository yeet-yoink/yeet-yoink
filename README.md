# yeet/yoink — A file storage and retrieval service

A service to which you can yeet your files in order to yoink them from somewhere else.
This is meant to simplify cluster-local file sharing with configurable persistence backends.

## HTTP API

### Health Checks

* `/startupz` - Meant for Kubernetes startup probes. 
* `/readyz` - Meant for Kubernetes readiness probes. 
* `/livez` - Meant for Kubernetes liveness probes. 
* `/health` - Meant for complete health checks (e.g. by Google Cloud Load Balancer). 
* `/healthz` - Meant for human inspection.
