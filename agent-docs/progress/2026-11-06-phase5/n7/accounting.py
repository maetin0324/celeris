"""N7: immutable N3 raw -> per-trial USD-equivalent table; no LLM/network."""
import json
from pathlib import Path
from decimal import Decimal

here = Path(__file__).resolve().parent
source = here.parent / 'n3'
rows = []
for path in sorted(source.glob('llm-*/raw/*.json')):
    raw = json.loads(path.read_text())
    if raw.get('tier') != 'llm':
        continue
    usage = raw.get('usage') or {}
    worker = raw.get('worker_usage') or {}
    keys = ('input_tokens', 'output_tokens', 'cache_read_tokens', 'cache_creation_tokens')
    missing = raw.get('usage_null_runs', 0) or raw.get('cost_unknown_runs', 0)
    complete = not missing and raw.get('cost_usd') is not None and all(usage.get(k) is not None for k in keys)
    if raw.get('worker_runs'):
        complete = complete and all(worker.get(k) is not None for k in keys)
    # N3 uses sonnet-5-5; N0 contract: write 2.5 -> 4 USD/M token.
    # This is a TTL sensitivity estimate, not per-run micro-USD rounded daemon accounting.
    creation = sum(u.get('cache_creation_tokens', 0) or 0 for u in (usage, worker)) if complete else None
    saved = Decimal(str(raw['cost_usd'])) if complete else None
    ttl = saved + Decimal(creation) * Decimal('0.0000015') if complete else None
    rows.append(dict(seq=raw['seq'], case=raw['case'], arm=raw['arm'], repeat=raw['repeat'],
                     saved_usd=float(saved) if saved is not None else None,
                     ttl_1h_usd=float(ttl) if ttl is not None else None,
                     cache_creation_tokens=creation, human_interventions=raw.get('human_interventions'),
                     recovery_rate=None, source=str(path.relative_to(here.parent))))
assert len(rows) == 32 and len({r['seq'] for r in rows}) == 32
rows.sort(key=lambda r: r['seq'])
(here / 'accounting.json').write_text(json.dumps(rows, indent=2, ensure_ascii=False) + '\n')
lines = ['# N3 実LLM既存32試行のUSD相当（N7で再集計）', '',
         '単価は凍結表のSonnet 5.5。保存値にcache creation × (4−2.5)/100万を加えた1時間TTL感度推定。請求額・daemonのrun単位切り上げ値ではない。',
         'usageはCoSとworker_usageを加算。回復率は全試行で未計測。介入は初回の人の発言を含む。相談12試行は採点未実施。', '',
         '| seq | 課題 | 腕 | 保存USD | 1h TTL USD | creation token | 人の介入 | 回復率 |',
         '|---:|---|---|---:|---:|---:|---:|---|']
for r in rows:
    money = lambda k: f"{r[k]:.6f}" if r[k] is not None else '不明'
    lines.append(f"| {r['seq']} | {r['case']} | {r['arm']} | {money('saved_usd')} | {money('ttl_1h_usd')} | {r['cache_creation_tokens']} | {r['human_interventions']} | 未計測 |")
for arm in ('old', 'new'):
    group = [r for r in rows if r['arm'] == arm]
    assert all(r['saved_usd'] is not None for r in group)
    lines.append(f"\n{arm}（{len(group)}試行）: 保存 {sum(r['saved_usd'] for r in group):.6f} USD、1h TTL {sum(r['ttl_1h_usd'] for r in group):.6f} USD、人の介入 {sum(r['human_interventions'] for r in group)} 回。")
(here / 'accounting.md').write_text('\n'.join(lines) + '\n')
print('\n'.join(lines[-4:]))
