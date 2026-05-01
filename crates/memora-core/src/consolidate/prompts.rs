pub const REGION_OVERVIEW_PROMPT: &str = r#"You write factual one-paragraph summaries of a region's content based on extracted claims. Output rules — follow exactly:

- Output exactly one paragraph, 2-4 sentences, max 600 characters.
- State what the region IS based only on the provided subjects and claims.
- Do not ask questions.
- Do not address the reader.
- Do not say "It seems" or "It appears" or "Let me know."
- Do not invent details not present in the input.
- If the input has fewer than 5 claims, output exactly: "Too few claims for synthesis."

GOOD example output:
Work tracking notes for the INTERNORGA 2026 trade fair, covering exhibitor activation metrics, kiosk builder development, and digital signage rollout. Decisions logged include rebuilding the kiosk interface from scratch after early UX feedback.

BAD example output (do NOT produce this style):
It seems like you're working on something related to trade fairs! Let me know if you'd like me to help with..."#;

pub const REGION_DESCRIPTION_PROMPT: &str = r#"You write one-line descriptions of a vault region for a directory listing. Output rules — follow exactly:

- Output exactly one sentence, max 120 characters.
- Describe what the region contains based on the provided subjects.
- Do not ask questions.
- Do not say "It seems" or "It appears."
- If the region name is "default" or appears to be a placeholder, output exactly: "General notes (uncategorized)."

GOOD: "Project notes for Memora — architecture decisions, build plan, and dogfooding logs."
BAD: "It seems like you're working on Memora! Here are some sample subjects...""#;

pub const WORLD_MAP_PROMPT: &str = r#"You write a 2-sentence overview of a vault's shape based on its region inventory. Output rules — follow exactly:

- Output exactly 2 sentences, max 300 characters.
- First sentence: total scale (count of notes and claims, dominant region).
- Second sentence: what kinds of regions are present (work, personal, reference, etc., based on names).
- Do not ask questions.
- Do not invent.
- If total claims across all regions is < 10, output exactly:
  "Vault is in early stages. Add more notes for synthesis to be meaningful."

GOOD example:
47 notes and 184 claims, concentrated in the work region. The vault spans professional projects, personal logs, and a reference library of papers."#;

/// Static overview when the vault has fewer than 10 total claims (see [`WORLD_MAP_PROMPT`]).
pub const WORLD_MAP_EARLY_STAGES_FALLBACK: &str =
    "Vault is in early stages. Add more notes for synthesis to be meaningful.";

pub const SUBREGION_PROPOSAL_PROMPT: &str = r#"You propose sub-regions for a region that has grown too large. Output must be valid JSON only, matching this schema:

{ "proposed_subregions": [ { "name": "...", "sample_subjects": [...], "claim_ids": [...] } ] }

Rules:
- Each name is 1-3 words, lowercase, hyphen-separated, suitable as a folder name. No "Untitled", no "Misc", no "Other".
- Each subregion must have at least 10 claim_ids.
- If you cannot propose at least 2 meaningful subregions, output { "proposed_subregions": [] }."#;
