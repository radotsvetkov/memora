//! `memora report` — a self-contained, offline HTML overview of a vault: summary
//! stats, an interactive claim graph, the contradictions/supersessions and stale
//! dependencies that need attention, and the world map. One file, no server, no
//! network, no CDN (system fonts only). All vault-derived text is HTML-escaped
//! and the embedded graph JSON is `\u`-escaped, so note content cannot inject
//! markup or break out of the data script.
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::Utc;
use clap::Args;
use serde::Serialize;

use memora_core::claims::{ClaimStore, Provenance, StalenessTracker};
use memora_core::{Claim, Memora};

use super::verdict;

/// Cap on graph nodes so the embedded force layout stays smooth in the browser.
const DEFAULT_MAX_NODES: usize = 400;

#[derive(Debug, Args)]
pub struct ReportArgs {
    /// Vault to report on.
    #[arg(long, default_value = "vault")]
    pub vault: PathBuf,
    /// Output HTML path (default: <vault>/.memora/report.html).
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// Open the report in your browser after writing it.
    #[arg(long, default_value_t = false)]
    pub open: bool,
    /// Maximum number of claims to draw in the graph.
    #[arg(long, default_value_t = DEFAULT_MAX_NODES)]
    pub max_nodes: usize,
}

#[derive(Serialize)]
struct GraphNode {
    id: String,
    label: String,
    region: String,
    kind: &'static str,
}

#[derive(Serialize)]
struct GraphEdge {
    source: String,
    target: String,
    rel: &'static str,
}

#[derive(Serialize)]
struct Graph {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    total_claims: usize,
}

pub async fn run(args: ReportArgs) -> Result<()> {
    let memora = Memora::open(&args.vault)?;
    let index = memora.index();
    let store = ClaimStore::new(index);
    let provenance = Provenance::new(index);
    let stale_tracker = StalenessTracker::new(index, &provenance);

    // Gather every claim, with its note's region.
    let mut note_ids = index.all_ids()?;
    note_ids.sort();
    let mut region_of: HashMap<String, String> = HashMap::new();
    let mut claims: Vec<Claim> = Vec::new();
    for note_id in &note_ids {
        let region = index
            .get_note(note_id)?
            .map(|row| row.region)
            .unwrap_or_default();
        region_of.insert(note_id.clone(), region);
        claims.extend(store.list_for_note(note_id)?);
    }

    let now = Utc::now();
    let contradictions = store.contradictions_unack()?;
    let stale = stale_tracker.list_stale()?;
    let stale_ids: HashSet<String> = stale.iter().map(|(id, _)| id.clone()).collect();
    let contradicted_ids: HashSet<String> = contradictions
        .iter()
        .flat_map(|(a, b)| [a.id.clone(), b.id.clone()])
        .collect();
    let superseded_ids: HashSet<String> = claims
        .iter()
        .filter(|c| c.valid_until.is_some_and(|until| until <= now))
        .map(|c| c.id.clone())
        .collect();
    let regions: BTreeSet<String> = region_of
        .values()
        .filter(|r| !r.is_empty())
        .cloned()
        .collect();

    let claim_label = |c: &Claim| -> String {
        let obj = c.object.as_deref().unwrap_or("");
        let raw = format!("{} {} {}", c.subject, c.predicate, obj);
        let raw = raw.trim();
        raw.chars().take(64).collect::<String>()
    };
    let kind_of = |id: &str| -> &'static str {
        if superseded_ids.contains(id) {
            "superseded"
        } else if contradicted_ids.contains(id) {
            "contradicted"
        } else if stale_ids.contains(id) {
            "stale"
        } else {
            "normal"
        }
    };

    // Provenance edges (source -> derived) among existing claims.
    let claim_ids: HashSet<String> = claims.iter().map(|c| c.id.clone()).collect();
    let mut prov_edges: Vec<(String, String)> = Vec::new();
    for claim in &claims {
        for source in provenance.sources_of(&claim.id)? {
            if claim_ids.contains(&source) {
                prov_edges.push((source, claim.id.clone()));
            }
        }
    }

    // Pick the nodes to draw: flagged and provenance/contradiction-connected
    // claims first (the interesting ones), then fill up to the cap.
    let connected: HashSet<String> = prov_edges
        .iter()
        .flat_map(|(s, d)| [s.clone(), d.clone()])
        .chain(contradicted_ids.iter().cloned())
        .collect();
    let is_priority = |id: &str| {
        superseded_ids.contains(id)
            || contradicted_ids.contains(id)
            || stale_ids.contains(id)
            || connected.contains(id)
    };
    let max_nodes = args.max_nodes.max(1);
    let mut ordered: Vec<&Claim> = claims.iter().filter(|c| is_priority(&c.id)).collect();
    ordered.extend(claims.iter().filter(|c| !is_priority(&c.id)));
    ordered.truncate(max_nodes);
    let node_ids: HashSet<String> = ordered.iter().map(|c| c.id.clone()).collect();

    let nodes: Vec<GraphNode> = ordered
        .iter()
        .map(|c| GraphNode {
            id: c.id.clone(),
            label: claim_label(c),
            region: region_of.get(&c.note_id).cloned().unwrap_or_default(),
            kind: kind_of(&c.id),
        })
        .collect();

    let mut edges: Vec<GraphEdge> = Vec::new();
    for (s, d) in &prov_edges {
        if node_ids.contains(s) && node_ids.contains(d) {
            edges.push(GraphEdge {
                source: s.clone(),
                target: d.clone(),
                rel: "derives",
            });
        }
    }
    for (a, b) in &contradictions {
        if node_ids.contains(&a.id) && node_ids.contains(&b.id) {
            edges.push(GraphEdge {
                source: a.id.clone(),
                target: b.id.clone(),
                rel: "contradicts",
            });
        }
    }

    let graph = Graph {
        nodes,
        edges,
        total_claims: claims.len(),
    };

    let world_map = fs::read_to_string(args.vault.join("world_map.md")).ok();

    let html = render_html(&ReportData {
        vault: args.vault.display().to_string(),
        generated: now.format("%Y-%m-%d %H:%M UTC").to_string(),
        n_claims: claims.len(),
        n_notes: note_ids.len(),
        n_regions: regions.len(),
        n_contradictions: contradictions.len(),
        n_stale: stale.len(),
        n_superseded: superseded_ids.len(),
        now,
        graph: &graph,
        contradictions: &contradictions,
        stale: &stale,
        world_map: world_map.as_deref(),
    });

    let out_path = args
        .output
        .unwrap_or_else(|| args.vault.join(".memora").join("report.html"));
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create report dir {}", parent.display()))?;
    }
    fs::write(&out_path, html).with_context(|| format!("write report {}", out_path.display()))?;

    println!("wrote report -> {}", out_path.display());
    println!(
        "  {} claims · {} notes · {} regions · {} contradictions · {} stale · {} superseded",
        graph.total_claims,
        note_ids.len(),
        regions.len(),
        contradictions.len(),
        stale.len(),
        superseded_ids.len()
    );
    if args.open {
        verdict::open_in_browser(&out_path);
    }
    Ok(())
}

struct ReportData<'a> {
    vault: String,
    generated: String,
    n_claims: usize,
    n_notes: usize,
    n_regions: usize,
    n_contradictions: usize,
    n_stale: usize,
    n_superseded: usize,
    now: chrono::DateTime<Utc>,
    graph: &'a Graph,
    contradictions: &'a [(Claim, Claim)],
    stale: &'a [(String, String)],
    world_map: Option<&'a str>,
}

/// Escape a JSON string for safe embedding inside a `<script>` block: neutralize
/// the only sequences that could close the tag or be parsed as markup.
fn json_for_script(json: &str) -> String {
    json.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

fn render_html(d: &ReportData<'_>) -> String {
    let esc = verdict::html_escape;
    let graph_json = serde_json::to_string(d.graph).unwrap_or_else(|_| "{}".to_string());
    let graph_json = json_for_script(&graph_json);

    let mut contradictions_html = String::new();
    if d.contradictions.is_empty() {
        contradictions_html.push_str("<p class=\"empty\">None detected.</p>");
    } else {
        for (a, b) in d.contradictions {
            // Determine which side is current. The superseded claim (valid_until in
            // the past) is the struck/older one; otherwise fall back to valid_from.
            let a_superseded = a.valid_until.is_some_and(|u| u <= d.now);
            let b_superseded = b.valid_until.is_some_and(|u| u <= d.now);
            let (older, newer) = if a_superseded && !b_superseded {
                (a, b)
            } else if b_superseded && !a_superseded {
                (b, a)
            } else if a.valid_from <= b.valid_from {
                (a, b)
            } else {
                (b, a)
            };
            contradictions_html.push_str(&format!(
                "<div class=\"row\"><div class=\"subj\">{}</div>\
                 <div class=\"pair\"><span class=\"bad\">{}</span> \
                 <span class=\"arrow\">→</span> <span class=\"ok\">{}</span></div>\
                 <div class=\"meta\">claim {} → {}</div></div>",
                esc(&newer.subject),
                esc(older.object.as_deref().unwrap_or("(none)")),
                esc(newer.object.as_deref().unwrap_or("(none)")),
                esc(&short_id(&older.id)),
                esc(&short_id(&newer.id)),
            ));
        }
    }

    let mut stale_html = String::new();
    if d.stale.is_empty() {
        stale_html.push_str("<p class=\"empty\">None.</p>");
    } else {
        for (id, reason) in d.stale {
            stale_html.push_str(&format!(
                "<div class=\"row\"><div class=\"meta\">claim {}</div>\
                 <div class=\"reason\">{}</div></div>",
                esc(&short_id(id)),
                esc(reason),
            ));
        }
    }

    let world_map_html = match d.world_map {
        Some(text) if !text.trim().is_empty() => {
            format!("<pre class=\"worldmap\">{}</pre>", esc(text))
        }
        _ => "<p class=\"empty\">No world_map.md yet. Run <code>memora challenge</code> to generate one.</p>"
            .to_string(),
    };

    let graph_section = if d.graph.nodes.is_empty() {
        "<p class=\"empty\">No claims indexed yet. Run <code>memora index</code> first.</p>"
            .to_string()
    } else {
        let truncated = if d.graph.total_claims > d.graph.nodes.len() {
            format!(
                "<p class=\"note\">Showing {} of {} claims (most connected first).</p>",
                d.graph.nodes.len(),
                d.graph.total_claims
            )
        } else {
            String::new()
        };
        format!(
            "{truncated}\
             <div class=\"graph-wrap\">\
               <canvas id=\"graph-canvas\"></canvas>\
               <div id=\"detail\" class=\"detail hidden\"></div>\
             </div>\
             <div class=\"legend\">\
               <span class=\"lg normal\">claim</span>\
               <span class=\"lg contradicted\">contradicted</span>\
               <span class=\"lg superseded\">superseded</span>\
               <span class=\"lg stale\">stale</span>\
               <span class=\"lg edge-derives\">derives</span>\
               <span class=\"lg edge-contradicts\">contradicts</span>\
             </div>"
        )
    };

    format!(
        "<!DOCTYPE html>\n<html lang=\"en\"><head>\n\
<meta charset=\"UTF-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n\
<title>memora report — {vault_title}</title>\n\
<style>{css}</style>\n\
</head><body>\n\
<header class=\"top\">\
  <div class=\"brand\"><span class=\"dot\"></span>memora <span class=\"sub\">report</span></div>\
  <div class=\"vault\">{vault}</div>\
</header>\n\
<section class=\"stats\">\
  <div class=\"stat\"><b>{n_claims}</b><span>claims</span></div>\
  <div class=\"stat\"><b>{n_notes}</b><span>notes</span></div>\
  <div class=\"stat\"><b>{n_regions}</b><span>regions</span></div>\
  <div class=\"stat bad\"><b>{n_contradictions}</b><span>contradictions</span></div>\
  <div class=\"stat warn\"><b>{n_superseded}</b><span>superseded</span></div>\
  <div class=\"stat violet\"><b>{n_stale}</b><span>stale</span></div>\
</section>\n\
<section><h2>Claim graph</h2>{graph_section}</section>\n\
<section><h2>Contradictions &amp; supersessions</h2>{contradictions_html}</section>\n\
<section><h2>Stale dependencies</h2>{stale_html}</section>\n\
<section><h2>World map</h2>{world_map_html}</section>\n\
<footer>Generated {generated} · provenance hash-proven · this file is fully offline.</footer>\n\
<script id=\"graph-data\" type=\"application/json\">{graph_json}</script>\n\
<script>{js}</script>\n\
</body></html>\n",
        vault_title = esc(&d.vault),
        vault = esc(&d.vault),
        css = REPORT_CSS,
        n_claims = d.n_claims,
        n_notes = d.n_notes,
        n_regions = d.n_regions,
        n_contradictions = d.n_contradictions,
        n_superseded = d.n_superseded,
        n_stale = d.n_stale,
        graph_section = graph_section,
        contradictions_html = contradictions_html,
        stale_html = stale_html,
        world_map_html = world_map_html,
        generated = esc(&d.generated),
        graph_json = graph_json,
        js = REPORT_JS,
    )
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

const REPORT_CSS: &str = r#"
:root{--bg:#0a0c10;--surface:#12151c;--border:#1f2630;--text:#e8ebf1;--dim:#9aa4b4;
--muted:#626c7e;--accent:#34d399;--red:#f87171;--amber:#fbbf24;--violet:#b3a4f7;
--sans:system-ui,-apple-system,Segoe UI,Roboto,sans-serif;--mono:ui-monospace,SFMono-Regular,Menlo,monospace;}
*{box-sizing:border-box;margin:0;padding:0}
body{background:var(--bg);color:var(--text);font-family:var(--sans);line-height:1.6;padding:0 0 64px}
.top{display:flex;align-items:center;justify-content:space-between;padding:18px 28px;border-bottom:1px solid var(--border);position:sticky;top:0;background:rgba(10,12,16,.85);backdrop-filter:blur(8px);z-index:5}
.brand{font-weight:700;font-size:19px;display:flex;align-items:center;gap:9px}
.brand .dot{width:9px;height:9px;border-radius:50%;background:var(--accent);box-shadow:0 0 10px rgba(52,211,153,.7)}
.brand .sub{color:var(--muted);font-weight:500;font-size:14px}
.vault{font-family:var(--mono);font-size:12.5px;color:var(--muted)}
.stats{display:flex;flex-wrap:wrap;gap:12px;padding:24px 28px}
.stat{background:var(--surface);border:1px solid var(--border);border-radius:12px;padding:14px 20px;min-width:108px}
.stat b{display:block;font-size:26px;font-family:var(--mono)}
.stat span{color:var(--dim);font-size:13px}
.stat.bad b{color:var(--red)}.stat.warn b{color:var(--amber)}.stat.violet b{color:var(--violet)}
section{padding:14px 28px 8px}
h2{font-size:17px;margin-bottom:12px;font-weight:600;letter-spacing:-.01em}
.empty,.note{color:var(--muted);font-size:14px}.note{margin-bottom:10px}
code{font-family:var(--mono);background:var(--surface);padding:1px 6px;border-radius:6px;font-size:13px}
.graph-wrap{position:relative;border:1px solid var(--border);border-radius:14px;overflow:hidden;background:#0c0f15}
#graph-canvas{display:block;width:100%;height:520px}
.detail{position:absolute;top:12px;right:12px;max-width:300px;background:var(--surface);border:1px solid var(--border);border-radius:10px;padding:12px 14px;font-size:13px}
.detail.hidden{display:none}
.detail .t{font-family:var(--mono);color:var(--accent);font-size:12px;margin-bottom:4px}
.detail .r{color:var(--dim);font-size:12px}
.legend{display:flex;flex-wrap:wrap;gap:8px 16px;padding:12px 2px;font-size:12.5px;color:var(--dim)}
.lg{display:inline-flex;align-items:center;gap:6px}
.lg::before{content:"";width:10px;height:10px;border-radius:50%}
.lg.normal::before{background:var(--accent)}.lg.contradicted::before{background:var(--red)}
.lg.superseded::before{background:var(--amber)}.lg.stale::before{background:var(--violet)}
.lg.edge-derives::before{border-radius:0;width:14px;height:2px;background:var(--muted)}
.lg.edge-contradicts::before{border-radius:0;width:14px;height:2px;background:var(--red)}
.row{border:1px solid var(--border);border-radius:10px;padding:11px 14px;margin-bottom:8px;background:var(--surface)}
.row .subj{font-weight:600;margin-bottom:3px}
.pair{font-family:var(--mono);font-size:13px}
.pair .bad{color:var(--red);text-decoration:line-through}
.pair .ok{color:var(--accent)}.pair .arrow{color:var(--muted)}
.row .meta{color:var(--muted);font-family:var(--mono);font-size:12px;margin-top:3px}
.row .reason{color:var(--dim);font-size:13px;margin-top:2px}
pre.worldmap{background:var(--surface);border:1px solid var(--border);border-radius:12px;padding:16px;overflow:auto;font-family:var(--mono);font-size:13px;color:var(--dim);white-space:pre-wrap;max-height:520px}
footer{color:var(--muted);font-size:12.5px;padding:24px 28px;border-top:1px solid var(--border);margin-top:24px}
"#;

const REPORT_JS: &str = r#"
(function(){
  var el=document.getElementById('graph-data');
  if(!el)return;
  var data=JSON.parse(el.textContent);
  var nodes=data.nodes||[], edges=data.edges||[];
  if(!nodes.length)return;
  var canvas=document.getElementById('graph-canvas');
  var ctx=canvas.getContext('2d');
  var detail=document.getElementById('detail');
  var colors={normal:'#34d399',contradicted:'#f87171',superseded:'#fbbf24',stale:'#b3a4f7'};
  function esc(s){return String(s==null?'':s).replace(/[<>&"]/g,function(c){return {'<':'&lt;','>':'&gt;','&':'&amp;','"':'&quot;'}[c];});}
  var byId={};
  function size(){
    var r=canvas.getBoundingClientRect();
    var dpr=window.devicePixelRatio||1;
    canvas.width=r.width*dpr; canvas.height=r.height*dpr;
    ctx.setTransform(dpr,0,0,dpr,0,0);
    return {w:r.width,h:r.height};
  }
  var dim={w:0,h:0};
  nodes.forEach(function(n){ byId[n.id]=n; });
  var links=edges.map(function(e){return {s:byId[e.source],t:byId[e.target],rel:e.rel};})
                 .filter(function(l){return l.s&&l.t;});
  function place(){
    nodes.forEach(function(n,i){
      var a=2*Math.PI*i/nodes.length;
      n.x=dim.w/2+Math.cos(a)*Math.min(dim.w,dim.h)*0.32;
      n.y=dim.h/2+Math.sin(a)*Math.min(dim.w,dim.h)*0.32;
    });
  }
  // force simulation with cooling
  var temp=1.0, ticks=0, MAX=320;
  function step(){
    var k=Math.sqrt((dim.w*dim.h)/Math.max(nodes.length,1))*0.55;
    for(var i=0;i<nodes.length;i++){
      var a=nodes[i]; a.fx=0; a.fy=0;
      for(var j=0;j<nodes.length;j++){
        if(i===j)continue;
        var b=nodes[j], dx=a.x-b.x, dy=a.y-b.y, d=Math.sqrt(dx*dx+dy*dy)||0.01;
        var rep=(k*k)/d; a.fx+=(dx/d)*rep; a.fy+=(dy/d)*rep;
      }
      // gravity to center
      a.fx+=(dim.w/2-a.x)*0.02; a.fy+=(dim.h/2-a.y)*0.02;
    }
    links.forEach(function(l){
      var dx=l.t.x-l.s.x, dy=l.t.y-l.s.y, d=Math.sqrt(dx*dx+dy*dy)||0.01;
      var att=(d*d)/k*0.5/d;
      l.s.fx+=dx*att*0.5; l.s.fy+=dy*att*0.5;
      l.t.fx-=dx*att*0.5; l.t.fy-=dy*att*0.5;
    });
    nodes.forEach(function(n){
      n.x+=Math.max(-12,Math.min(12,n.fx*temp));
      n.y+=Math.max(-12,Math.min(12,n.fy*temp));
      n.x=Math.max(14,Math.min(dim.w-14,n.x));
      n.y=Math.max(14,Math.min(dim.h-14,n.y));
    });
    temp*=0.985; ticks++;
  }
  function draw(){
    ctx.clearRect(0,0,dim.w,dim.h);
    links.forEach(function(l){
      ctx.beginPath(); ctx.moveTo(l.s.x,l.s.y); ctx.lineTo(l.t.x,l.t.y);
      ctx.strokeStyle=l.rel==='contradicts'?'rgba(248,113,113,.55)':'rgba(98,108,126,.35)';
      ctx.lineWidth=l.rel==='contradicts'?1.6:1; ctx.stroke();
    });
    nodes.forEach(function(n){
      ctx.beginPath(); ctx.arc(n.x,n.y,5,0,2*Math.PI);
      ctx.fillStyle=colors[n.kind]||colors.normal; ctx.fill();
    });
  }
  function loop(){ if(ticks<MAX){ step(); draw(); requestAnimationFrame(loop);} else { draw(); } }
  // Start only once the canvas has a real layout size. End-of-body scripts can
  // run before the first layout pass, which would otherwise leave the graph
  // blank. A ResizeObserver is the reliable signal; rAF + window resize are
  // fallbacks. Once started, later size changes just resize and redraw.
  var started=false;
  function tryStart(){
    if(started) return;
    dim=size();
    if(dim.w && dim.h){ started=true; place(); temp=1.0; ticks=0; loop(); }
  }
  if(typeof ResizeObserver!=='undefined'){
    new ResizeObserver(function(){ if(!started){ tryStart(); } else { dim=size(); draw(); } }).observe(canvas);
  }
  tryStart();
  (function poll(){ if(started) return; tryStart(); if(!started) requestAnimationFrame(poll); })();
  function pick(mx,my){
    var best=null,bd=14;
    nodes.forEach(function(n){var d=Math.hypot(n.x-mx,n.y-my); if(d<bd){bd=d;best=n;}});
    return best;
  }
  canvas.addEventListener('click',function(ev){
    var r=canvas.getBoundingClientRect();
    var n=pick(ev.clientX-r.left, ev.clientY-r.top);
    if(n){
      detail.innerHTML='<div class="t">'+esc(n.id.slice(0,16))+'</div><div>'+
        esc(n.label)+'</div><div class="r">'+esc(n.region)+' · '+esc(n.kind)+'</div>';
      detail.classList.remove('hidden');
    } else { detail.classList.add('hidden'); }
  });
  window.addEventListener('resize',function(){ if(!started){ tryStart(); return; } dim=size(); draw(); });
})();
"#;
