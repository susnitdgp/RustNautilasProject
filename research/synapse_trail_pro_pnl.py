#!/usr/bin/env python3
"""Synapse Trail Pro v1.4 default-signal research reproduction. Research-only; no orders."""
import json,datetime as dt,collections,math
COST=2.; SLIP=.5
def study(tf):
 raw=json.load(open(f'/tmp/amd_crudeoil_{tf}m.json'))
 b=[dict(t=dt.datetime.strptime(x['timestamp'][:19],'%Y-%m-%dT%H:%M:%S'),o=x['open'],h=x['high'],l=x['low'],c=x['close']) for x in raw]
 ema=None;atr=None;lo=up=None;direction=0;signals=[]
 for i,x in enumerate(b):
  prev=b[i-1]['c'] if i else x['c'];tr=max(x['h']-x['l'],abs(x['h']-prev),abs(x['l']-prev))
  # Seed ATR like ta.atr: first Wilder ATR after 13 true range observations
  if i==0:trs=[]
  trs.append(tr)
  if len(trs)<13:atr=0.
  elif len(trs)==13:atr=sum(trs)/13
  else:atr=(atr*12+tr)/13
  ema=x['c'] if ema is None else ema+(x['c']-ema)*2/22
  lowraw=ema-atr*1.618;upraw=ema+atr*1.618
  old=direction;previous_lo=lo if lo is not None else x['c'];previous_up=up if up is not None else x['c']
  if i>=55:
   if direction==1 and x['c']<previous_lo:direction=-1
   elif direction==-1 and x['c']>previous_up:direction=1
   elif direction==0:
    if x['c']>previous_up:direction=1
    elif x['c']<previous_lo:direction=-1
  flipped=direction!=old
  if direction==1:lo=lowraw if flipped else max(lowraw,lo if lo is not None else lowraw);up=upraw
  elif direction==-1:up=upraw if flipped else min(upraw,up if up is not None else upraw);lo=lowraw
  else:lo=lowraw;up=upraw
  if (old==-1 and direction==1) or (old==1 and direction==-1):signals.append((i,direction,atr))
 return b,signals
def simulate(b,signals,mode):
 pending={i+1:(d,a) for i,d,a in signals if i+1<len(b)}
 trades=[];p=None
 for i,x in enumerate(b):
  # Pending signal first: next candle's open. Realistic assumed execution
  if i in pending:
   d,atr=pending[i]
   if p is not None:
    px=x['o']-p['d']*SLIP
    trades.append({'day':str(p['t'].date()),'net':sum(((p['tp1'] if p['hit1'] and k==1 else p['tp2'] if p['hit2'] and k==2 else px)-p['entry'])*p['d']/3 for k in (1,2,3))-COST,'reason':'flip'})
    p=None
   if atr>0 and i>0 and x['t'].date()==b[i-1]['t'].date() and x['t'].hour*60+x['t'].minute<1335:
    entry=x['o']+d*SLIP;risk=round(atr*1.5)
    p={'t':x['t'],'i':i,'d':d,'entry':entry,'risk':risk,'stop':entry-d*risk,'tp1':entry+d*risk,'tp2':entry+d*risk*2,'tp3':entry+d*risk*3,'hit1':False,'hit2':False,'be_at':None}
  if p is None:continue
  d=p['d'];exitpx=None;reason=None
  if x['t'].date()!=p['t'].date() or x['t'].hour*60+x['t'].minute>=1395:
   exitpx=b[i-1]['c']-d*SLIP;reason='session'
  elif i>p['i']:
   slhit=x['l']<=p['stop'] if d==1 else x['h']>=p['stop']
   t3hit=x['h']>=p['tp3'] if d==1 else x['l']<=p['tp3']
   if slhit and not (x['o']>=p['tp3'] if d==1 else x['o']<=p['tp3']):
    exitpx=(min(x['o'],p['stop']) if d==1 else max(x['o'],p['stop']))-d*SLIP;reason='stop'
   elif t3hit:
    p['hit1']=True;p['hit2']=True
    exitpx=p['tp3']-d*SLIP;reason='tp3'
   else:
    if not p['hit1'] and (x['h']>=p['tp1'] if d==1 else x['l']<=p['tp1']):p['hit1']=True;p['be_at']=i+1
    if not p['hit2'] and (x['h']>=p['tp2'] if d==1 else x['l']<=p['tp2']):p['hit2']=True
    if p['be_at'] is not None and i>=p['be_at']:p['stop']=p['entry']
  if exitpx is not None:
   prices=[p['tp1'] if p['hit1'] else exitpx,p['tp2'] if p['hit2'] else exitpx,p['tp3'] if reason=='tp3' else exitpx]
   net=sum((q-p['entry'])*d/3 for q in prices)-COST
   trades.append({'day':str(p['t'].date()),'net':net,'reason':reason});p=None
 return trades
def month(ts,s):
 x=[v['net'] for v in ts if v['day'].startswith(s)];wins=sum(t>0 for t in x);gp=sum(max(0,t) for t in x);gl=-sum(min(0,t) for t in x)
 return dict(trades=len(x),wins=wins,net_points=round(sum(x),2),profit_factor=round(gp/gl,3) if gl else None)
if __name__=='__main__':
 for tf in (3,5):
  b,s=study(tf);t=simulate(b,s,'default')
  print(json.dumps({'tf':tf,'bars':len(b),'flip_signals':len(s),'sept':month(t,'2026-09'),'oct':month(t,'2026-10'),'exits':dict(collections.Counter(x['reason'] for x in t))}))
