# 使い方: python3 -I summarize.py  （before/ after/ の samples*.jsonl から傾き・tick・dispatch 遅延を出す）
import json,glob,statistics as st
for f in sorted(glob.glob('*/samples*.jsonl')):
    R=[json.loads(l) for l in open(f)]
    p=[(r['t']/60,r['proc']['rss_anon_kb']/1024) for r in R if r['t']>=60]
    n=len(p);mx=sum(a for a,_ in p)/n;my=sum(b for _,b in p)/n
    s=sum((a-mx)*(b-my) for a,b in p)/sum((a-mx)**2 for a,_ in p)
    d=R[-1]['t']-R[0]['t']; ticks=R[-1]['daemon']['ticks']
    from collections import Counter
    base=Counter(R[0]['db'].get('dispatch_latency_ms',[]))
    allc=Counter()
    for r in R: allc|=Counter(r['db'].get('dispatch_latency_ms',[]))
    lat=sorted((allc-base).elements())  # soak 中に新たに出た値だけ（写した DB の履歴を除く）
    fr=R[-1]['process_memory']['malloc_free']/2**20
    print(f"{f}: dur={d/60:.1f}min slope(t>=1min)={s:.2f}MB/min rss_anon_end={R[-1]['proc']['rss_anon_kb']/1024:.0f}MB malloc_free_end={fr:.0f}MB ticks/min={ticks/(d/60):.1f} slow_ticks={R[-1].get('slow_ticks')} new_dispatch_n={len(lat)} new_dispatch_ms={lat}")
