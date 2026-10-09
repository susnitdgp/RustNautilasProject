#!/usr/bin/env python3
"""1m CRUDEOIL VWAP pullback versus EMA crossover; costed research, not live orders."""
import json,datetime as dt,collections,math,statistics
COST=2.;SLIP=.5
raw=json.load(open('/tmp/amd_crudeoil_1m.json'))
b=[dict(t=dt.datetime.strptime(x['timestamp'][:19],'%Y-%m-%dT%H:%M:%S'),o=x['open'],h=x['high'],l=x['low'],c=x['close'],v=x['volume']) for x in raw]
e9=e21=None; atr=None; rtr=0.;pdm=mdm=trr=adx=None;dxseed=[];plusseed=[];minusseed=[];trseed=[]
vsum=csum=0.;volwindow=collections.deque(maxlen=20);lastday=None;signals={k:[] for k in ('vwap','ema')}
for i,x in enumerate(b):
 day=x['t'].date()
 if day!=lastday:vsum=csum=0.;lastday=day
 vsum+=x['v'];csum+=x['c']*x['v']
 vwap=csum/vsum if vsum>0 else None
 pe9=e9;pe21=e21
 e9=x['c'] if e9 is None else e9+(x['c']-e9)*.2
 e21=x['c'] if e21 is None else e21+(x['c']-e21)*(2/22)
 pc=b[i-1]['c'] if i else x['c']
 tr=max(x['h']-x['l'],abs(x['h']-pc),abs(x['l']-pc))
 up=x['h']-b[i-1]['h'] if i else 0.;down=b[i-1]['l']-x['l'] if i else 0.
 dp=up if up>down and up>0 else 0.;dm=down if down>up and down>0 else 0.
 trseed.append(tr);plusseed.append(dp);minusseed.append(dm)
 if i==13:atr=sum(trseed[-14:])/14;pdm=sum(plusseed[-14:])/14;mdm=sum(minusseed[-14:])/14;trr=atr
 elif i>13:
  atr=(atr*13+tr)/14;pdm=(pdm*13+dp)/14;mdm=(mdm*13+dm)/14;trr=(trr*13+tr)/14
  dplus=100*pdm/trr if trr else 0.;dminus=100*mdm/trr if trr else 0.
  dx=100*abs(dplus-dminus)/(dplus+dminus) if dplus+dminus else 0.
  dxseed.append(dx)
  if len(dxseed)==14:adx=sum(dxseed)/14
  elif len(dxseed)>14:adx=(adx*13+dx)/14
 volwindow.append(x['v'])
 x.update(e9=e9,e21=e21,atr=atr,vwap=vwap,adx=adx)
 if i<220 or i+1>=len(b) or adx is None:continue
 nextbar=b[i+1]
 if nextbar['t'].date()!=day:continue
 minute=x['t'].hour*60+x['t'].minute
 if minute<555 or minute>=1325:continue
 if (pe9 is not None and pe21 is not None):
  if pe9<=pe21 and e9>e21:signals['ema'].append((i,1))
  if pe9>=pe21 and e9<e21:signals['ema'].append((i,-1))
 # Require pullback into EMA9-EMA21 zone sometime in last 3 candles; price holds VWAP
 if vwap is None or adx<=22:continue
 for d in (1,-1):
  if (e9-e21)*d<=0 or (x['c']-vwap)*d<=0:continue
  if (x['c']-e9)*d<=0 or (x['c']-x['o'])*d<=0:continue
  if any((y['c']-y['vwap'])*d<=0 for y in b[i-3:i] if y['vwap'] is not None):continue
  pulled=any((y['l']<=y['e9'] if d==1 else y['h']>=y['e9']) and ((y['c']-y['vwap'])*d>0 if y['vwap'] else False) for y in b[i-3:i])
  if not pulled:continue
  if (x['h']-x['l'])>1.8*atr:continue
  signals['vwap'].append((i,d))
def trade(signals,rr):
 pending={i+1:(i,d) for i,d in signals}
 active=None;trades=[];losses=collections.Counter()
 for i,x in enumerate(b):
  day=x['t'].date()
  if active:
   p=active;d=p['d'];px=None;reason=None
   if day!=p['day']:
    px=b[i-1]['c']-d*SLIP;reason='session'
   elif i>p['entry_i']:
    if x['l']<=p['stop'] if d==1 else x['h']>=p['stop']:
     px=(min(x['o'],p['stop']) if d==1 else max(x['o'],p['stop']))-d*SLIP;reason='stop'
    elif x['h']>=p['target'] if d==1 else x['l']<=p['target']:
     px=(max(x['o'],p['target']) if d==1 else min(x['o'],p['target']))-d*SLIP;reason='target'
    elif i-p['entry_i']>=10:
     px=x['c']-d*SLIP;reason='timeout'
   if px is not None:
    net=(px-p['entry'])*d-COST
    trades.append(dict(day=str(p['day']),net=net,reason=reason,entry_i=p['entry_i'],exit_i=i))
    losses[day]=losses[day]+1 if net<0 else 0
    active=None
  if active is None and i in pending:
   src,d=pending[i]
   if b[src]['t'].date()!=day or x['t'].hour*60+x['t'].minute>=1335 or losses[day]>=3:continue
   entry=x['o']+d*SLIP;atr=b[src]['atr']
   if not atr:continue
   risk=min(atr,max(.5*atr,abs(entry-(min(b[k]['l'] for k in range(max(0,src-5),src+1)) if d==1 else max(b[k]['h'] for k in range(max(0,src-5),src+1))))))
   active=dict(d=d,entry=entry,stop=entry-d*risk,target=entry+d*rr*risk,entry_i=i,day=day)
 return trades
def summary(trades,month):
 a=[x for x in trades if x['day'].startswith(month)]
 gp=sum(max(0,x['net']) for x in a);gl=-sum(min(0,x['net']) for x in a)
 daily=collections.defaultdict(float)
 for x in a:daily[x['day']]+=x['net']
 equity=peak=draw=0
 for _,v in sorted(daily.items()):
  equity+=v;peak=max(peak,equity);draw=max(draw,peak-equity)
 return dict(trades=len(a),wins=sum(x['net']>0 for x in a),net_pts=round(sum(x['net'] for x in a),2),pf=round(gp/gl,3) if gl else None,dd=round(draw,2))
if __name__=='__main__':
 for k,v in signals.items():
  print('SIGNALS',k,len(v))
  for rr in (1.,1.5,2.):
   t=trade(v,rr)
   print(json.dumps(dict(model=k,rr=rr,sept=summary(t,'2026-09'),oct=summary(t,'2026-10'))))
