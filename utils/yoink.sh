#!/usr/bin/env bash

set -euo pipefail

host="localhost"
port="8080"
file_id="${1:-}"

if [ -z "$file_id" ]
then
  echo "Please provide file_id as an argument."
  exit 1
fi

response=$(curl -i -X GET "http://$host:$port/yoink/$file_id")

echo "$response"
