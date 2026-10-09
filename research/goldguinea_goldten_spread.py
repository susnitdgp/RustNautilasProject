#!/usr/bin/env python3
"""Research-only close-price pair spread mean reversion; not executable bid/ask arbitrage."""
import json,statistics as st,collections
A=json.load(open('/tmp/goldguinea_oct_3m.json')); B={r['timestamp']:r for r in json.load(open('/tmp/goldten_oct_3m.json'))}
rows=[]
for a in A:
 b=B.get(a['timestamp'])
 if b:rows.append({'time':a['timestamp'],'spread':5*a['close']-4*b['close'],'va':a['volume'],'vb':b['volume']})
def test(zentry,lookback=120,hold=20,expense=0):
 trades=[];active=None
 for i in range(lookback,len(rows)-1):
  r=rows[i];s=r['spread'];sample=[v['spread'] for v in rows[i-lookback:i]]
  mu=st.mean(sample);sigma=st.pstdev(sample)
  if sigma<=0:continue
  z=(s-mu)/sigma
  if active:
   d,idx,entry,day=active
   if (d*(s-mu)>=0 or i-idx>=hold or day!=r['time'][:10]):
    # Entry/exit at next candle close proxy; ignores price impact; expense is cost per 40g *round trip*
    p=rows[i+1]['spread']
    trades.append({'date':day,'net':d*(p-entry)-expense,'bars':i-idx});active=None
  elif abs(z)>=zentry and r['va']>0 and r['vb']>0 and rows[i+1]['time'][:10]==r['time'][:10]:
   # spread above mean => short spread, below => long spread
   active=(-1 if z>0 else 1,i,rows[i+1]['spread'],r['time'][:10])
 return trades
def stat(trades,month):
 t=[x for x in trades if x['date'].startswith(month)]
 if not t:return (0,0,0)
 return len(t),round(sum(x['net'] for x in t),2),round(sum(x['net']>0 for x in t)/len(t),3)
for z in (1.5,2.0,2.5,3.0):
 t=test(z)
 print('Z',z,'0friction',stat(t,'2026-09'),stat(t,'2026-10'))
 for expense in (100,250,500):
  a=[dict(x,net=x['net']-expense) for x in t]
  print(' cost40g',expense,'SEP',stat(a,'2026-09'),'OCT',stat(a,'2026-10'))
