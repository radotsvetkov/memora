# Deterministic, no-API-key core metric: how many fabricated citations does
# memora reject vs. a naive (unverified) pipeline. Doubles as a CI gate.
bench:
	cargo run -p memora-bench --release --bin bench_citation_rejection

# Lists the LoCoMo fixture queries (not scored — see the binary's note).
bench-locomo:
	cargo run -p memora-bench --release --bin bench_locomo

.PHONY: bench bench-locomo
