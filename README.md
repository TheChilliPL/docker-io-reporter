# Docker IO reporter

Reports Docker container IO stats in a Prometheus-compatible manner.

## Docker container

The repo contains a Dockerfile that can be used to run it.

It automatically hosts a Prometheus-compatible server at 0.0.0.0:9100.
The IP and port can be changed using `DOCKER_IO_REPORTER_IP` and `DOCKER_IO_REPORTER_PORT` enviroment variables.
