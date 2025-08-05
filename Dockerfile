FROM rust:1-alpine

RUN apk update && apk add musl-dev

WORKDIR /app
COPY . .

RUN cargo install --locked --path .

CMD ["docker-io-reporter", "host"]
