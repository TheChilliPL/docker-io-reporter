# Docker IO reporter

Reports Docker container IO stats in a Prometheus-compatible manner.

## Docker container

The repo contains a Dockerfile that can be used to run it.

It automatically hosts a Prometheus-compatible server at 0.0.0.0:9100.
The IP and port can be changed using `DOCKER_IO_REPORTER_IP` and `DOCKER_IO_REPORTER_PORT` enviroment variables.
Obviously you can also just bind the port to another one in Docker.

Minimal Docker Compose example:
```yml
services:
  docker-io-reporter:
    build: ./docker-io-reporter
    restart: unless-stopped
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock:ro # Docker socket communication
      - /proc:/proc # Process information access
    cgroup: host # Forces /sys/fs/cgroup sharing
    ports:
      - 9100:9100
```
