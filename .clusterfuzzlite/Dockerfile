FROM gcr.io/oss-fuzz-base/base-builder-rust:v1
RUN apt-get update && apt-get install -y --no-install-recommends zip
COPY . $SRC/mzstd
WORKDIR $SRC/mzstd
COPY .clusterfuzzlite/build.sh $SRC/
