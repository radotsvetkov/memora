# Ingesting documents

Memora verifies citations against the byte spans of markdown notes. To make an
external document verifiable, you turn it into a vault note first. `memora ingest`
does that: it extracts clean text from the source, writes a note with valid
frontmatter under a region you choose, and then the normal pipeline (index →
extract claims → verify) treats it like any other note.

```bash
memora ingest meeting-notes.txt        --vault ~/brain
memora ingest interview.vtt            --vault ~/brain --region interviews
memora ingest contract.pdf             --vault ~/brain --region legal   # needs the pdf feature
memora ingest https://example.com/post --vault ~/brain --region web     # needs the web feature
```

After ingesting, index the vault so the claims become verifiable:

```bash
memora index --vault ~/brain
```

## Supported formats

| Format | Extensions | Notes |
|---|---|---|
| Plain text | `.txt`, `.text` | Read as-is. |
| Markdown | `.md`, `.markdown` | Read as-is. |
| Transcripts | `.vtt`, `.srt` | Cue numbers, timestamps, and the `WEBVTT` header are stripped; spoken text is kept. |
| PDF | `.pdf` | Text extraction via `pdf-extract`. Requires the `pdf` feature. |
| Web page | a URL, or `.html`/`.htm` | Readable text (paragraphs, headings, lists, quotes, code) and the page title, via `scraper`. Scripts, styles, and most navigation are dropped. Requires the `web` feature. |

## Optional features (PDF and web)

PDF and web support are behind Cargo features so the default binary and its supply
chain stay lean. Enable what you need:

```bash
cargo install memora-cli --features pdf        # PDF
cargo install memora-cli --features web        # URLs and .html files
cargo install memora-cli --features "pdf web"  # both
```

Without the matching feature, `memora ingest` fails with a clear message rather
than silently doing nothing. Notes:

- Scanned (image-only) PDFs have no extractable text; run OCR first and ingest the result.
- Web extraction is best-effort; it keeps the main content but may miss or include
  some chrome. Edit the resulting note in Obsidian to trim anything unwanted before indexing.

## What the note looks like

- **id** — a readable slug from the filename or URL plus a short hash of the
  source, so re-ingesting the same source updates the same note instead of
  duplicating it.
- **source** — `reference` (an external document, not your own writing).
- **region** — `--region` (default `ingested`).
- **privacy** — `--privacy` (default `private`); use `secret` for sensitive
  documents so their content is redacted before any cloud call.
- **summary** — the first non-empty line, falling back to the filename.

The body is the extracted text, lightly normalized (control characters removed,
long runs of blank lines collapsed). You can edit it in Obsidian afterward like
any other note.
