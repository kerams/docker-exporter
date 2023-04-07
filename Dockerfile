from scratch
arg DIR
copy ${DIR}/docker-exporter /
expose 9417
entrypoint [ "/docker-exporter" ]