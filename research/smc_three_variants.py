#!/usr/bin/env python3
"""SMC 3-variant hypothesis: causal pivot structure, OB retest and FVG. No broker orders."""
import json,datetime as dt,collections,math
COST=2.;SLIP=.5
def load(tf):
 a=json.load(open(f'/tmp/amd_crudeoil_{tf}m.json'))
 return [dict(t=dt.datetime.strptime(v['timestamp'][:19],'%Y-%m-%dT%H:%M:%S'),o=v['open'],h=v['high'],l=v['low'],c=v['close']) for v in a]
def test(tf,variant,rr):
 b=load(tf); hi=lo=None; trend=0;lastChoch=None;lastBos=None;candidate=None;signals=[];events=collections.Counter();atr=0.;fvg=[]
 # HTF 15-minute regime: last closed 15m bar close vs last closed 15m 20-EMA, avoids peeking at current bar
 htf=[];groups=collections.OrderedDict()
 for x in b:groups.setdefault((x['t'].date(),(x['t'].hour*60+x['t'].minute-540)//15),[]).append(x)
 for vals in groups.values():
  if len(vals)==15//tf:htf.append((vals[-1]['t'],vals[-1]['c']))
 htfema=None;htf_idx=0;bias=0
 for i,x in enumerate(b):
  while htf_idx<len(htf) and htf[htf_idx][0]<x['t']:
   htfema=htf[htf_idx][1] if htfema is None else htfema+(htf[htf_idx][1]-htfema)*2/21
   bias=1 if htf[htf_idx][1]>htfema else -1;htf_idx+=1
  prev=b[i-1]['c'] if i else x['c'];tr=max(x['h']-x['l'],abs(x['h']-prev),abs(x['l']-prev));atr=tr if i==0 else (atr*13+tr)/14
  if i>=10:
   j=i-5
   if b[j]['h']>max(z['h'] for z in b[j-5:j]) and b[j]['h']>=max(z['h'] for z in b[j+1:i+1]):hi=b[j]['h']
   if b[j]['l']<min(z['l'] for z in b[j-5:j]) and b[j]['l']<=min(z['l'] for z in b[j+1:i+1]):lo=b[j]['l']
  if i<220 or hi is None or lo is None:continue
  if i>=2:
   if x['l']>b[i-2]['h']:fvg.append((i,1,b[i-2]['h'],x['l']))
   if x['h']<b[i-2]['l']:fvg.append((i,-1,x['h'],b[i-2]['l']))
  fvg=fvg[-100:]
  d=0
  if i>0 and b[i-1]['c']<=hi and x['c']>hi:d=1
  elif i>0 and b[i-1]['c']>=lo and x['c']<lo:d=-1
  if d:
   if trend!=d:lastChoch=(i,d);lastBos=None;events['choch']+=1
   elif lastChoch and lastChoch[1]==d and i-lastChoch[0]<=30:
    lastBos=(i,d);events['bos_after_choch']+=1
    if variant== 'A' and bias==d:signals.append((i,d,atr,lo if d==1 else hi))
    if variant in ('B','C'):
     # latest opposite candle preceding BOS serves as a simplified internal order-block proxy
     others=[k for k in range(max(lastChoch[0],i-12),i) if ((b[k]['c']<b[k]['o']) if d==1 else (b[k]['c']>b[k]['o']))]
     if others:
      k=others[-1];candidate=(i,d,b[k]['l'],b[k]['h'],atr)
   trend=d
  if candidate and i>candidate[0] and i-candidate[0]<=20:
   start,d,zlo,zhi,a=candidate
   if ((x['l']<zlo-0.2*a) if d==1 else (x['h']>zhi+0.2*a)):candidate=None
   elif bias==d and ((x['l']<=zhi and x['c']>(zlo+zhi)/2) if d==1 else (x['h']>=zlo and x['c']<(zlo+zhi)/2)):
    align=any(f[1]==d and f[0]>=start-8 and f[0]<=i and (f[3]>=zlo if d==1 else f[2]<=zhi) for f in fvg)
    if variant=='B' or align:signals.append((i,d,a,zlo if d==1 else zhi));events['retest']+=1
    candidate=None
   elif i-candidate[0]>=20:candidate=None
  elif candidate and i-candidate[0]>20:candidate=None
 pending={i+1:(i,d,a,stop) for i,d,a,stop in signals if i+1<len(b)}
 active=None;trades=[]
 for i,x in enumerate(b):
  if active:
   p=active;d=p['dir'];price=None;reason=None
   if x['t'].date()!=p['day']:
    price=b[i-1]['c']-d*SLIP;reason='session'
   elif i>p['i']:
    if x['l']<=p['stop'] if d==1 else x['h']>=p['stop']:
     price=(min(x['o'],p['stop']) if d==1 else max(x['o'],p['stop']))-d*SLIP;reason='stop'
    elif x['h']>=p['tp'] if d==1 else x['l']<=p['tp']:
     price=(max(x['o'],p['tp']) if d==1 else min(x['o'],p['tp']))-d*SLIP;reason='target'
    elif i-p['i']>=30//tf:
     price=x['c']-d*SLIP;reason='timeout'
   if price is not None:
    trades.append(dict(day=str(p['day']),net=(price-p['entry'])*d-COST,reason=reason))
    active=None
  if active is None and i in pending:
   src,d,a,stop=pending[i]
   if x['t'].date()!=b[src]['t'].date() or x['t'].hour*60+x['t'].minute>=1335:continue
   entry=x['o']+d*SLIP
   risk=d*(entry-stop)+.2*a
   if not (.3*a<=risk<=1.5*a):continue
   active=dict(day=x['t'].date(),dir=d,entry=entry,stop=entry-d*risk,tp=entry+d*risk*rr,i=i)
 return trades,events,len(signals)
def summary(ts,mo):
 p=[x['net'] for x in ts if x['day'].startswith(mo)];gp=sum(max(0,v) for v in p);gl=-sum(min(0,v) for v in p)
 return dict(trades=len(p),wins=sum(v>0 for v in p),net_points=round(sum(p),2),profit_factor=round(gp/gl,3) if gl else None)
if __name__=='__main__':
 for tf in (3,5):
  for kind in ('A','B','C'):
   for rr in (1.5,2):
    ts,events,signals=test(tf,kind,rr)
    print(json.dumps(dict(tf=tf,variant=kind,rr=rr,signals=signals,events=dict(events),sept=summary(ts,'2026-09'),oct=summary(ts,'2026-10'))))
