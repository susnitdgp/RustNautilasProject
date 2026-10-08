#!/usr/bin/env python3
"""Research reproduction of LuxAlgo structure events; hypothetical execution, no broker."""
import json,datetime as dt,collections,os
COST=2.0;SLIP=0.5
def run(tf,kind):
 raw=json.load(open(f'/tmp/amd_crudeoil_{tf}m.json'))
 bars=[dict(t=dt.datetime.strptime(x['timestamp'][:19],'%Y-%m-%dT%H:%M:%S'),o=x['open'],h=x['high'],l=x['low'],c=x['close']) for x in raw]
 # LuxAlgo swings: os flips only when historical price len bars ago exceeds all following len highs/lows.
 def swings(length):
  trend=0;prev=0;top=None;btm=None;out=[]
  for i,b in enumerate(bars):
   t=bb=None
   if i>=length:
    if bars[i-length]['h']>max(x['h'] for x in bars[i-length+1:i+1]):trend=0
    elif bars[i-length]['l']<min(x['l'] for x in bars[i-length+1:i+1]):trend=1
    if trend==0 and prev!=0:t=bars[i-length]['h']
    if trend==1 and prev!=1:bb=bars[i-length]['l']
   if t is not None:top=t
   if bb is not None:btm=bb
   out.append((t,bb,top,btm))
   prev=trend
  return out
 long=swings(50);short=swings(3)
 osdir=0;top_cross=False;bot_cross=False;sbtm_cross=False;stop_cross=False
 topy=btmy=stopy=sbtmy=None;maxv=minv=None
 candidates=[];events=collections.Counter()
 for i,b in enumerate(bars):
  top,btm,topy0,btmy0=long[i];stop,sbtm,stopy0,sbtmy0=short[i]
  if top is not None:topy=top;top_cross=False
  if btm is not None:btmy=btm;bot_cross=False
  before=osdir
  if topy is not None and b['c']>topy and not top_cross:osdir=1;top_cross=True
  if btmy is not None and b['c']<btmy and not bot_cross:osdir=0;bot_cross=True
  if osdir!=before:maxv=b['h'];minv=b['l'];sbtm_cross=False;stop_cross=False;events['choch']+=1
  if stop is not None:stopy=stop
  if sbtm is not None:sbtmy=sbtm
  # replicate original order: IDM then BOS then sweeps; rolling max/min updated at end
  signal=None
  if osdir==1:
   if sbtmy is not None and btmy is not None and b['l']<sbtmy and not sbtm_cross and sbtmy!=btmy:
    sbtm_cross=True;events['bull_inducement']+=1
   if maxv is not None and b['c']>maxv and sbtm_cross:
    sbtm_cross=False;events['bull_bos']+=1
    if kind=='bos':signal=(1,minv)
   if maxv is not None and b['h']>maxv and b['c']<maxv and kind=='sweep':
    signal=(-1,b['h']);events['high_sweep']+=1
  else:
   if stopy is not None and topy is not None and b['h']>stopy and not stop_cross and stopy!=topy:
    stop_cross=True;events['bear_inducement']+=1
   if minv is not None and b['c']<minv and stop_cross:
    stop_cross=False;events['bear_bos']+=1
    if kind=='bos':signal=(-1,maxv)
   if minv is not None and b['l']<minv and b['c']>minv and kind=='sweep':
    signal=(1,b['l']);events['low_sweep']+=1
  if maxv is not None:maxv=max(maxv,b['h'])
  if minv is not None:minv=min(minv,b['l'])
  if signal and i+1<len(bars):
   direction,stopv=signal
   candidates.append((i,direction,stopv))
 # next-open, 2R target, one position, session day flat, max 12 bars; assume stop before target
 trades=[];active=None;sig={i+1:(direction,stopv,i) for i,direction,stopv in candidates}
 for i,b in enumerate(bars):
  if active:
   p=active;direction=p['d']
   if b['t'].date()!=p['day'] or b['t'].hour*60+b['t'].minute>=1395:
    price=bars[i-1]['c']-direction*SLIP;reason='session'
   elif i>p['i'] and ((b['l']<=p['stop']) if direction==1 else (b['h']>=p['stop'])):
    price=(min(b['o'],p['stop']) if direction==1 else max(b['o'],p['stop']))-direction*SLIP;reason='stop'
   elif i>p['i'] and ((b['h']>=p['target']) if direction==1 else (b['l']<=p['target'])):
    price=(max(b['o'],p['target']) if direction==1 else min(b['o'],p['target']))-direction*SLIP;reason='target'
   elif i-p['i']>=12:
    price=b['c']-direction*SLIP;reason='timeout'
   else:price=None
   if price is not None:
    trades.append(dict(date=str(p['day']),net=(price-p['entry'])*direction-COST,reason=reason))
    active=None
  if active is None and i in sig:
   d,stop,origin=sig[i]
   if bars[origin]['t'].date()!=b['t'].date() or b['t'].hour*60+b['t'].minute>=1395:continue
   entry=b['o']+d*SLIP
   risk=(entry-stop)*d
   if risk<=0 or risk>250:events['invalid_risk']+=1;continue
   active={'d':d,'stop':stop,'target':entry+d*2*risk,'entry':entry,'i':i,'day':b['t'].date()}
 result={}
 for month in ('2026-09','2026-10'):
  x=[t for t in trades if t['date'].startswith(month)]
  gp=sum(max(0,t['net']) for t in x);gl=-sum(min(0,t['net']) for t in x)
  pnl=sum(t['net'] for t in x)
  result[month]=dict(trades=len(x),wins=sum(t['net']>0 for t in x),net_points=pnl,profit_factor=gp/gl if gl else None)
 return {'timeframe':tf,'setup':kind,'counts':dict(events),'results':result,'total_trades':len(trades)}
if __name__=='__main__':
 for tf in (3,5):
  for kind in ('bos','sweep'):
   print(json.dumps(run(tf,kind)))
