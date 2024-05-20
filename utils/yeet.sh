#!/usr/bin/env bash

set -euo pipefail

host="localhost"
port="8080"
timestamp=$(date +%s)

response=$(curl -i -X POST "http://$host:$port/yeet?file_name=test-dynamic.json" \
     -H "Content-Type: application/json" \
     -d '{
  "some": "field",
  "ts": "'"$timestamp"'"
}')

headers=$(echo "$response" | awk '/HTTP\/1.1/,/^\r/{print}')
json=$(echo "$response" | awk '/^\{/,EOF{print}')

# Print the entire response
echo "Headers: $headers"
echo "Body: $json"
echo

# Using 'jq' command-line JSON processor for parsing JSON response
id=$(echo "$json" | jq -r '.id')
file_size=$(echo "$json" | jq -r '.file_size_bytes')
sha256=$(echo "$json" | jq -r '.hashes.sha256')
md5=$(echo "$json" | jq -r '.hashes.md5')

# Print Specific Values
echo "ID: $id"
echo "File Size: $file_size"
echo "SHA-256 Hash: $sha256"
echo "MD5 Hash: $md5"
echo

# Using 'awk' command for processing the response headers
yy_id=$(echo "$headers" | awk -F: '/yy-id/ {print $2}')

# Print header value
echo "yy-id header: $yy_id"
