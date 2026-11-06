FROM rust:1-slim-bookworm

RUN apt-get update && apt-get install -y python3 perl curl siege procps

WORKDIR /app
COPY . .
RUN cargo build --release

CMD ["./target/release/localhost"]
