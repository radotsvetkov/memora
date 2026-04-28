pub const REGION_OVERVIEW_PROMPT: &str = r#"You are writing an atlas overview for a knowledge region.
Input: a list of tuples [claim_id, subject, predicate, object].
Output rules:
- one paragraph only
- plain text only
- maximum 600 characters
- capture the main shape of the region facts
- no markdown, no bullets"#;

pub const REGION_DESCRIPTION_PROMPT: &str = r#"You are naming a region in one compact line.
Input: region name and sample subjects.
Output rules:
- exactly one line
- plain text only
- maximum 120 characters
- describe what this region is about for quick scanning"#;

pub const WORLD_MAP_PROMPT: &str = r#"You are writing a world map overview for a knowledge vault.
Input: list of regions with short descriptions and counts.
Output rules:
- start with a 2-3 sentence vault overview
- then include one concise one-liner per region
- plain text only
- no markdown headings"#;

pub const SUBREGION_PROPOSAL_PROMPT: &str = r#"You are splitting an oversized region into subregions.
Input: 200+ claims with subjects and current region.
Output JSON exactly:
{
  "proposed_subregions": [
    {
      "name": "string",
      "sample_subjects": ["string"],
      "claim_ids": ["string"]
    }
  ]
}
Rules:
- valid JSON only
- include 2-6 meaningful subregions
- each claim id can appear in at most one proposed subregion
- sample_subjects should be representative, short, and non-empty"#;
