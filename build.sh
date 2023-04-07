#!/bin/sh

GIT_REPO=$(basename `git rev-parse --show-toplevel`)
GIT_COMMIT=$(git rev-parse HEAD)
BUILD_TIME=$(date -u +"%Y-%m-%dT%H-%M-%SZ")
IMAGE_NAME="$GIT_REPO:$BUILD_TIME"

cargo b -r --target x86_64-unknown-linux-musl
#upx target/x86_64-unknown-linux-musl/release/docker-exporter
docker build -t $IMAGE_NAME --build-arg DIR=target/x86_64-unknown-linux-musl/release --label GIT_COMMIT=$GIT_COMMIT .