"""Parse the TrackConstants block-table lambda (output of ilmeth.py) into blocks.json.
Usage: python ilmeth.py "<.cctor>b__48_*" > cctor_l.txt && python blocks.py"""
import re, json, collections
L=open('cctor_l.txt').read().split('\n')
# split per lambda
lams=collections.OrderedDict(); cur=None
for l in L:
    if '-- method' in l: cur=l.split('-- method')[1].strip(); lams[cur]=[]
    elif cur: 
        p=l.split(None,2)
        if len(p)>=2: lams[cur].append((p[1], p[2].strip() if len(p)>2 else ''))
def ival(op,arg):
    if op.startswith('ldc.i4.m1'): return -1
    m=re.match(r'ldc\.i4\.(\d)$',op)
    if m: return int(m.group(1))
    if op in('ldc.i4.s','ldc.i4'): return int(arg)
    return None
for name, ins in lams.items():
    chunks=[]; c=[]
    for op,arg in ins:
        c.append((op,arg))
        if op=='callvirt' and arg.endswith('::Add'): chunks.append(c); c=[]
    print('##',name,'instr',len(ins),'blocks',len(chunks))
    tails=collections.Counter()
    for ch in chunks:
        # tail after last stelem
        idx=max([i for i,(o,a) in enumerate(ch) if o=='stelem'] or [-1])
        tails[' '.join(o+(':'+a if a and not a.startswith('M:') else '') for o,a in ch[idx+1:])]+=1
    for t,n in tails.most_common(8): print('   ',n,t[:200])
    if not chunks: print('   ', ins[:30])

ins=lams['<.cctor>b__48_1']
chunks=[]; c=[]
for op,arg in ins:
    c.append((op,arg))
    if op=='callvirt' and arg.endswith('::Add'): chunks.append(c); c=[]
BLOCKS=[]
for ch in chunks:
    conns=[]; i=0
    while i<len(ch):
        op,arg=ch[i]
        if op=='ldsfld' and arg.startswith('F:') and arg[2:] in ('TL','TM','TR','BL','BM','BR','LT','LM','LB','RT','RM','RB'):
            t=ival(*ch[i+1]); assert ch[i+2][1]=='M:Invoke', ch[i:i+3]
            conns.append((arg[2:],t)); i+=3; continue
        i+=1
    idx=max(i for i,(o,a) in enumerate(ch) if o=='stelem')
    ints=[ival(o,a) for o,a in ch[idx+1:] if ival(o,a) is not None]
    BLOCKS.append({'atlas':ints[0],'alt':ints[1],'conns':conns,'extra':ints[2:]})
print(len(BLOCKS))
from collections import Counter
print(Counter(len(b['conns']) for b in BLOCKS), Counter(t for b in BLOCKS for _,t in b['conns']), Counter(tuple(b['extra']) for b in BLOCKS))
by=collections.defaultdict(list)
for b in BLOCKS: by[b['atlas']].append(b)
for a in sorted(by): print(a, [(b['alt'],[f"{c}{'' if t==0 else ':'+str(t)}" for c,t in b['conns']]) for b in by[a]])
json.dump(BLOCKS, open('blocks.json','w'))
