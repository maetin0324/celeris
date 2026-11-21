# 「env が無いと return して pass」の試験の候補を出す（userns-skip の before で使った走査）。
# 使い方: python3 -I agent-docs/progress/2026-11-21-strategy-v1-1/userns-skip/scan-env-return.py "$PWD"
# 出力: file<TAB>試験名<TAB>return の行<TAB>判定に使った式。vendor/ は除く。stderr に env を読む helper の一覧。
import re, os, sys, subprocess
root=sys.argv[1]
files=[f for f in subprocess.run(["git","-C",root,"ls-files","crates/*.rs"],capture_output=True,text=True).stdout.split() if '/vendor/' not in f]
ENV=re.compile(r'env::var(_os)?\(|\bvar_os\(')
fn_re=re.compile(r'^\s*(pub(\([^)]*\))?\s+)?(async\s+)?fn\s+(\w+)')
def fns(lines):
    for i,l in enumerate(lines):
        m=fn_re.match(l)
        if not m: continue
        j=i-1; attrs=[]
        while j>=0 and lines[j].strip().startswith(('#[','//')): attrs.append(lines[j].strip()); j-=1
        is_test=any(re.match(r'#\[(tokio::)?test\b',a) for a in attrs)
        d=0; st=False; k=i
        while k<len(lines):
            for ch in lines[k]:
                if ch=='{': d+=1; st=True
                elif ch=='}': d-=1
            if st and d<=0: break
            if not st and lines[k].rstrip().endswith(';'): break
            k+=1
        yield m.group(4),i,min(k,len(lines)-1),is_test
GLOBAL={'skip_unless_userns_tests','skip_by_default'}
rows=[]; helpers=[]
for f in files:
    lines=open(os.path.join(root,f),encoding='utf-8',errors='replace').read().split('\n')
    local=set(GLOBAL)
    for l in lines:
        m=re.search(r'skip_unless_userns_tests\s+as\s+(\w+)',l)
        if m: local.add(m.group(1))
    allf=list(fns(lines))
    for _ in range(2):
        for name,s,e,t in allf:
            if t: continue
            body='\n'.join(lines[s+1:e+1]); sig=lines[s]+lines[s+1] if s+1<len(lines) else lines[s]
            if (ENV.search(body) or any(re.search(r'\b'+g+r'\s*\(',body) for g in local-{name})) and re.search(r'->\s*(bool|Option<)',sig) and name not in local:
                local.add(name); helpers.append((f,name,s+1))
    gre=re.compile(r'\b('+'|'.join(sorted(local))+r')\s*\(') 
    for name,s,e,t in allf:
        if not t: continue
        for li in range(s+1,e+1):
            if not re.search(r'\breturn\b',lines[li]): continue
            ctx=[lines[x] for x in range(max(s+1,li-3),li+1)]
            ctx=[c for c in ctx if re.search(r'\b(if|let|match)\b|Err|None|return',c)]
            txt='\n'.join(ctx)
            m=ENV.search(txt) or gre.search(txt)
            if m: rows.append((f,name,li+1,m.group(0).rstrip('(').strip())); break
for r in rows: print('\t'.join(map(str,r)))
for h in helpers: print('HELPER\t'+'\t'.join(map(str,h)),file=sys.stderr)
