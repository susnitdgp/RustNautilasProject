#!/usr/bin/env python3
"""Research-only AMD Po3 15m approximation (not byte-for-byte TradingView execution)."""
import json,datetime as D,math,collections,os
SLIP=float(os.environ.get("AMD_SLIPPAGE",0.5))
COST=float(os.environ.get("AMD_COST",2.0))
assert 0<=SLIP<=10 and 0<=COST<=20
from pathlib import Path
raw=json.load(open('/tmp/amd_crudeoil_5m.json'))
parse=lambda x:D.datetime.strptime(x['timestamp'][:19],'%Y-%m-%dT%H:%M:%S')
groups=collections.OrderedDict()
for x in raw:
 t=parse(x);key=(str(t.date()),(t.hour*60+t.minute-540)//15)
 groups.setdefault(key,[]).append(x)
b=[]
for vals in groups.values():
 if len(vals)!=3:continue
 if any((parse(vals[k])-parse(vals[0])).total_seconds()!=300*k for k in range(3)):continue
 b.append(dict(t=parse(vals[0]),o=vals[0]['open'],h=max(v['high'] for v in vals),l=min(v['low'] for v in vals),c=vals[-1]['close']))
assert len(b)>2000
widths=[];atr=0;prev=None;ph=[];pl=[];state='idle';last_end=-99;R={};sw={};pos=None;trades=[];cnt=collections.Counter()
def exit_trade(i,px,reason):
 global pos,state,last_end
 p=pos;points=(px-p['entry'])*p['dir']-COST
 trades.append(dict(day=str(p['time'].date()),time=str(p['time']),end=str(b[i]['t']),dir=p['dir'],entry=p['entry'],exit=px,net=points,reason=reason))
 cnt[reason]+=1;pos=None;state='idle';last_end=i
for i,x in enumerate(b):
 tr=max(x['h']-x['l'],abs(x['h']-prev),abs(x['l']-prev)) if prev is not None else x['h']-x['l']
 atr=tr if i==0 else (atr*13+tr)/14;prev=x['c']
 if i>=6:
  k=i-3
  if b[k]['h']>=max(v['h'] for v in b[k-3:k+4]):ph.append((k,b[k]['h']))
  if b[k]['l']<=min(v['l'] for v in b[k-3:k+4]):pl.append((k,b[k]['l']))
 ph=ph[-60:];pl=pl[-60:]
 sample=b[max(0,i-19):i+1];w=max(y['h'] for y in sample)-min(y['l'] for y in sample);widths.append(w)
 if pos:
  p=pos
  if x['t'].date()!=p['time'].date() or x['t'].hour*60+x['t'].minute>=1395:
   exit_trade(i,b[i-1]['c']-SLIP*p['dir'],'SESSION');continue
  if i>p['idx']:
   stop=x['l']<=p['stop'] if p['dir']>0 else x['h']>=p['stop']
   target=x['h']>=p['target'] if p['dir']>0 else x['l']<=p['target']
   if stop:
    px=min(x['o'],p['stop']) if p['dir']>0 else max(x['o'],p['stop'])
    exit_trade(i,px-SLIP*p['dir'],'STOP');continue
   if target:
    px=max(x['o'],p['target']) if p['dir']>0 else min(x['o'],p['target'])
    exit_trade(i,px-SLIP*p['dir'],'TARGET');continue
   if i-p['signal']>=64:exit_trade(i,x['c']-SLIP*p['dir'],'TIMEOUT');continue
  continue
 if state=='order':
  if x['t'].date()!=R['day'] or x['t'].hour*60+x['t'].minute>=1395:
   state='idle';last_end=i;cnt['ENTRY_EXPIRED']+=1;continue
  d=R['dir'];entry=x['o']+SLIP*d
  if (entry-R['stop'])*d<=0 or (R['target']-entry)*d<=0:
   state='idle';last_end=i;cnt['BAD_OPEN']+=1;continue
  pos={'time':x['t'],'dir':d,'entry':entry,'stop':R['stop'],'target':R['target'],'idx':i,'signal':i-1}
  state='dist';cnt['ENTRIES']+=1;continue
 if i<220:continue
 if state=='idle':
  if i-last_end<10 or sum(z<=w for z in widths[-200:])>50 or w<.0015*x['c']:continue
  hi=max(v['h'] for v in sample);lo=min(v['l'] for v in sample);start=i-19
  # Impulse-tail trim with minimum 12-bar range maturity.
  while i-start+1>12:
   sub=b[start+1:i+1];hi2=max(v['h'] for v in sub);lo2=min(v['l'] for v in sub)
   if (hi-lo)-(hi2-lo2)>.15*(hi-lo):hi,lo,start=hi2,lo2,start+1
   else:break
  highs=[v for j,v in ph if j>=start];lows=[v for j,v in pl if j>=start]
  hi=max(highs) if highs else hi;lo=min(lows) if lows else lo
  if lo<=x['c']<=hi and hi-lo>=.0015*x['c']:
   R={'hi':hi,'lo':lo,'tol':.1*(hi-lo),'width':hi-lo,'start':start,'atr':atr}
   state='accum';cnt['RANGES']+=1
 elif state=='accum':
  if i-R['start']>96:state='idle';last_end=i;cnt['EXPIRED']+=1;continue
  above=x['h']>R['hi']+R['tol'];below=x['l']<R['lo']-R['tol']
  if above and below:state='idle';last_end=i;cnt['BOTH']+=1;continue
  if above or below:
   if i-R['start']<12:state='idle';last_end=i;cnt['EARLY']+=1;continue
   sw={'side':1 if above else -1,'ext':x['h'] if above else x['l'],'index':i}
   state='pending';cnt['SWEEPS']+=1
   if R['lo']<=x['c']<=R['hi']:state='confirm'
 elif state=='pending':
  sw['ext']=max(sw['ext'],x['h']) if sw['side']>0 else min(sw['ext'],x['l'])
  if (x['l']<R['lo']-R['tol'] if sw['side']>0 else x['h']>R['hi']+R['tol']):
   state='idle';last_end=i;cnt['TWO_SIDED']+=1;continue
  if i-sw['index']>6:state='idle';last_end=i;cnt['BREAKOUT']+=1;continue
  if R['lo']<=x['c']<=R['hi']:state='confirm'
 if state=='confirm':
  d=-sw['side'];stop=math.floor(sw['ext']-.4*R['atr']) if d==1 else math.ceil(sw['ext']+.4*R['atr'])
  leg=R['hi']-sw['ext'] if d==1 else sw['ext']-R['lo']
  target=round(R['hi']+.5*leg) if d==1 else round(R['lo']-.5*leg)
  if (x['c']-stop)*d>0 and (target-x['c'])*d>=1:
   R={'dir':d,'stop':stop,'target':target,'day':x['t'].date()}
   state='order';cnt['CONFIRMED']+=1
  else:state='idle';last_end=i;cnt['GEOMETRY']+=1
months={}
for month in ['2026-09','2026-10']:
 a=[t for t in trades if t['day'].startswith(month)]
 pnl=collections.defaultdict(float)
 for t in a:pnl[t['day']]+=t['net']
 equity=high=dd=0.
 for day,v in sorted(pnl.items()):equity+=v;high=max(high,equity);dd=max(dd,high-equity)
 gp=sum(max(t['net'],0) for t in a);gl=-sum(min(t['net'],0) for t in a)
 months[month]={'trades':len(a),'wins':sum(t['net']>0 for t in a),'net_points':sum(t['net'] for t in a),'profit_factor':gp/gl if gl else None,'daily_drawdown':dd,'daily':dict(pnl)}
result={'contract':'CRUDEOIL26OCTFUT.MCX','15m_bars':len(b),'events':dict(cnt),'months':months,'trades':trades,'model':f'AMD Po3 default-rule approximation; next 15m open entry; {SLIP} slippage per side; {COST} points round trip cost','limitations':'Not exact TradingView replay; approximated pivots, ATR reference and percentile rank; OHLC stop ordering and session closes not broker fills'}
print(json.dumps(result,indent=2))
