#!/bin/sh

GIT_REPO=$(basename `git rev-parse --show-toplevel`)
GIT_COMMIT=$(git rev-parse HEAD)
BUILD_TIME=$(date -u +"%Y-%m-%dT%H-%M-%SZ")
IMAGE_NAME="keramss/$GIT_REPO:$BUILD_TIME"

cargo b -r --target x86_64-unknown-linux-musl
cargo b -r --target aarch64-unknown-linux-musl

docker build -t $IMAGE_NAME-amd64 --build-arg DIR=target/x86_64-unknown-linux-musl/release --platform linux/amd64 --label GIT_COMMIT=$GIT_COMMIT .
docker build -t $IMAGE_NAME-arm64 --build-arg DIR=target/aarch64-unknown-linux-musl/release --platform linux/arm64 --label GIT_COMMIT=$GIT_COMMIT .

docker push $IMAGE_NAME-amd64
docker push $IMAGE_NAME-arm64

docker rmi $IMAGE_NAME-amd64 $IMAGE_NAME-arm64

docker manifest create $IMAGE_NAME \
    --amend $IMAGE_NAME-amd64 \
    --amend $IMAGE_NAME-arm64

docker manifest push --purge $IMAGE_NAME