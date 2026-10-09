#!/usr/bin/env python3
"""VCE-Mojo v1.6 independent default signal/next-bar backtest; no order placement."""
import json,datetime as D,collections,statistics as S,math,os
COST=2.0;SLIP=.5
def run(tf,exit_r=1.0):
 raw=json.load(open(f"/tmp/{os.environ.get('VCE_DATASET','amd_crudeoil')}_{tf}m.json"))
 raw=[x for x in raw if x['timestamp'][:10]<='2026-10-08']
 b=[dict(t=D.datetime.strptime(x['timestamp'][:19],'%Y-%m-%dT%H:%M:%S'),o=x['open'],h=x['high'],l=x['low'],c=x['close']) for x in raw]
 tr=[];atr14=[];atr20=[];atr4=[];q14=q20=q4=None
 for i,x in enumerate(b):
  pc=b[i-1]['c'] if i else x['c'];v=max(x['h']-x['l'],abs(x['h']-pc),abs(x['l']-pc));tr.append(v)
  if i==13:q14=sum(tr[:14])/14
  elif i>13:q14=(13*q14+v)/14
  if i==19:q20=sum(tr[:20])/20
  elif i>19:q20=(19*q20+v)/20
  if i==3:q4=sum(tr[:4])/4
  elif i>3:q4=(3*q4+v)/4
  atr14.append(q14);atr20.append(q20);atr4.append(q4)
 compLen=0;compHigh=compLow=None;violations=0;watch=None;trade=None;last_outcome=-100
 trades=[];counts=collections.Counter();sessBars=0
 for i,x in enumerate(b):
  if i<70:continue
  day=x['t'].date()
  newSession=i==0 or b[i-1]['t'].date()!=day
  if newSession:compLen=0;compHigh=compLow=None;violations=0;watch=None;sessBars=1
  else:sessBars+=1
  A=atr14[i];BG=atr20[i];L=atr4[i]
  # Pine: session high/low computed from current bar, including opening bar only on second bar.
  rangeLen=max(1,min(50,sessBars-1))
  session=b[max(0,i-rangeLen+1):i+1]
  shi=max(v['h'] for v in session);slo=min(v['l'] for v in session)
  sr=shi-slo
  medianrng=S.median(v['h']-v['l'] for v in b[i-49:i+1])
  contract=(L < BG*.82 or x['h']-x['l']<medianrng*.65)
  catalyst=any(b[i-k]['h']-b[i-k]['l']>atr20[i-k]*1.5 for k in range(1,6) if atr20[i-k] is not None)
  contract=contract or (catalyst and L<BG*1.10)
  if contract:
   if compLen==0:compLen=1;compHigh=x['h'];compLow=x['l'];violations=0
   else:
    tolerance=BG*.5
    within=x['h']<=compHigh+tolerance and x['l']>=compLow-tolerance
    if within:compLen+=1;compHigh=max(compHigh,x['h']);compLow=min(compLow,x['l'])
    else:
     violations+=1
     if violations<=2:compLen+=1;compHigh=max(compHigh,x['h']);compLow=min(compLow,x['l'])
     else:compLen=1;compHigh=x['h'];compLow=x['l'];violations=0
  else:
   if compLen>0:compLen-=1
   if compLen==0:compHigh=compLow=None;violations=0
  if compLen>14:compLen=0;compHigh=compLow=None;violations=0
  qualified=compLen>=4
  atHigh=qualified and compHigh is not None and compHigh>=shi-sr*.35
  atLow=qualified and compLow is not None and compLow<=slo+sr*.35
  if watch is None and sessBars>15:
   if atHigh:watch={'dir':-1,'high':compHigh,'low':compLow,'end':None}
   elif atLow:watch={'dir':1,'high':compHigh,'low':compLow,'end':None}
  if watch is not None:
   if qualified and ((watch['dir']==-1 and atHigh) or (watch['dir']==1 and atLow)):
    watch['high']=max(watch['high'],compHigh);watch['low']=min(watch['low'],compLow)
   if not qualified and watch['end'] is None:watch['end']=i
   if watch['end'] is not None and i-watch['end']>10:watch=None
  # Position exit evaluated on current OHLC, never on entry bar; EOD at bar close.
  # All real entries on following open, with cost/slip; signal-bar reference stop is shifted by open gap.
  if trade:
   d=trade['dir'];stop=trade['stop'];target=trade['target']
   if i>trade['entry_idx']:
    stopHit=x['l']<=stop if d==1 else x['h']>=stop
    targetHit=x['h']>=target if d==1 else x['l']<=target
    reason=None;price=None
    if stopHit:price=(min(x['o'],stop) if d==1 else max(x['o'],stop));reason='SL'
    elif targetHit:price=(max(x['o'],target) if d==1 else min(x['o'],target));reason='TP'
    elif x['t'].hour*60+x['t'].minute+tf>=1395:price=x['c'];reason='EOD'
    if reason:
     pnl=d*(price-d*SLIP-trade['entry'])-COST
     trades.append({'date':str(trade['t'].date()),'net':pnl,'reason':reason,'direction':d})
     counts[reason]+=1;trade=None;last_outcome=i
  # EOD signal blocked; next-bar open is used for executable entry
  if watch and trade is None and i-last_outcome>3:
   d=watch['dir']
   sh=watch['high'];sl=watch['low']
   doSignal=((x['c']<sl or (x['l']<sl and x['c']<x['o'] and (x['o']-x['c'])>(x['h']-x['l'])*.4)) if d==-1
         else (x['c']>sh or (x['h']>sh and x['c']>x['o'] and (x['c']-x['o'])>(x['h']-x['l'])*.4)))
   if doSignal and x['t'].hour*60+x['t'].minute+tf<1395 and i+1<len(b) and b[i+1]['t'].date()==day:
    entry=b[i+1]['o']+d*SLIP
    rawstop=(sl-A*.20) if d==1 else (sh+A*.20)
    refdist=min(A*2,max(A*.3,abs(x['c']-rawstop)))
    # use candle reference risk distance, next open executable entry
    trade={'dir':d,'entry':entry,'entry_idx':i+1,'t':b[i+1]['t'],'stop':entry-d*refdist,'target':entry+d*refdist*exit_r}
    counts['LONG' if d==1 else 'SHORT']+=1;watch=None
 return trades,counts,len(b)
def report(trades,month):
 t=[a['net'] for a in trades if a['date'].startswith(month)];gp=sum(max(0,v) for v in t);gl=-sum(min(0,v) for v in t)
 daily=collections.defaultdict(float)
 for a in trades:
  if a['date'].startswith(month):daily[a['date']]+=a['net']
 eq=peak=dd=0
 for val in daily.values():eq+=val;peak=max(peak,eq);dd=max(dd,peak-eq)
 return {'trades':len(t),'wins':sum(v>0 for v in t),'net_points':round(sum(t),2),'profit_factor':round(gp/gl,3) if gl else None,'max_daily_drawdown':round(dd,2)}
if __name__=='__main__':
 for tf in (1,3,5):
  for R in (1.,1.5,2.):
   trades,counts,n=run(tf,R)
   print(json.dumps({'tf':tf,'exit_R':R,'bars':n,'counts':dict(counts),'sept':report(trades,'2026-09'),'oct':report(trades,'2026-10')}))
