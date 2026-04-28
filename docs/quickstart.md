# Quickstart (10 minutes)

## 1) Build

```bash
cargo build --release
```

## 2) Initialize a vault

```bash
./target/release/memora init --vault ./vault
```

## 3) Index notes

```bash
./target/release/memora index --vault ./vault
```

## 4) Query

```bash
./target/release/memora query "What changed this week?" --vault ./vault
```

Raw note retrieval only:

```bash
./target/release/memora query --raw "What changed this week?" --vault ./vault
```

## 5) Run consolidation and challenger

```bash
./target/release/memora consolidate --all --vault ./vault
./target/release/memora challenge --vault ./vault
```

## 6) Run MCP server

```bash
./target/release/memora serve
```

Set env vars to point MCP tools at a custom vault:

```bash
export MEMORA_VAULT=./vault
export MEMORA_INDEX_DB=./vault/.memora/memora.db
export MEMORA_VECTOR_INDEX=./vault/.memora/vectors
```
