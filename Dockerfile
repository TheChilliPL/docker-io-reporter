FROM rust:1-alpine

WORKDIR /app
COPY . .

RUN cargo install --locked --path .

CMD ["docker-io-reporter", "host"]
