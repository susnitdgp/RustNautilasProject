#!/usr/bin/env python3
"""Dumb Money Concepts v6 signal event study; independent hypothetical trade rules."""
import json, collections, datetime as dt, math
from pathlib import Path
COST=2.0; SLIPPAGE=0.5
def moving_rma(previous, value, n):
    return value if previous is None else (previous*(n-1)+value)/n
def compute(tf):
    src=json.loads(Path(f"/tmp/amd_crudeoil_{tf}m.json").read_text())
    bars=[dict(t=dt.datetime.strptime(c['timestamp'][:19],'%Y-%m-%dT%H:%M:%S'),
       o=float(c['open']),h=float(c['high']),l=float(c['low']),c=float(c['close']),v=float(c['volume'])) for c in src]
    assert len(bars)>500
    last_close=None;atr=None;avg_gain=avg_loss=None;ema=None;adx=None;plus=None;minus=None;dxtr=None
    prev_dumb=None;prev_high=None;prev_low=None;dumb_ohlc=[];adxs=[];dis=[];events=collections.defaultdict(list)
    last_signal=collections.defaultdict(lambda:-10000)
    for i,b in enumerate(bars):
        c=b['c']; previous=bars[i-1]['c'] if i else c
        true_range=max(b['h']-b['l'],abs(b['h']-previous),abs(b['l']-previous))
        atr=moving_rma(atr,true_range,14)
        ema=c if ema is None else (c-ema)*2/21+ema
        gain=max(c-previous,0.);loss=max(previous-c,0.)
        avg_gain=moving_rma(avg_gain,gain,14);avg_loss=moving_rma(avg_loss,loss,14)
        rsi=100 if avg_loss==0 else 100-100/(1+avg_gain/avg_loss)
        volma=sum(x['v'] for x in bars[max(0,i-29):i+1])/min(i+1,30)
        # Dumb price and DDX closely follow the pasted Pine calculations
        vb=max(0,b['v']/max(volma,1)-1)
        bull=max(0,(rsi-70)/30)+(max(0,c-ema)/max(atr,1e-9))+vb
        bear=max(0,(30-rsi)/30)+(max(0,ema-c)/max(atr,1e-9))+vb
        dumb_c=(bull-bear)*atr
        dumb_o=prev_dumb if prev_dumb is not None else 0.0
        spread=abs(bull-bear)*atr*.3
        dumb_h=max(dumb_c,dumb_o)+spread;dumb_l=min(dumb_c,dumb_o)-spread
        if prev_high is not None:
            up=dumb_h-prev_high;down=prev_low-dumb_l
            fac=max(.85,min(1.15,.85+.3*min(b['v']/max(volma,1),1)))
            pdm=up*fac if up>down and up>0 else 0.
            mdm=down*fac if down>up and down>0 else 0.
            tr=max(dumb_h-dumb_l,abs(dumb_h-prev_dumb),abs(dumb_l-prev_dumb))
            plus=moving_rma(plus,pdm,14);minus=moving_rma(minus,mdm,14);dxtr=moving_rma(dxtr,tr,14)
            dp=100*plus/max(dxtr,1e-10);dm=100*minus/max(dxtr,1e-10)
            dx=100*abs(dp-dm)/max(dp+dm,1e-10)
            adx=moving_rma(adx,dx,14);adxs.append(adx);dis.append((dp,dm))
        else:adxs.append(None);dis.append((None,None))
        prev_high,prev_low,prev_dumb=dumb_h,dumb_l,dumb_c
        if i<250:continue
        def add(name,d):
            if i-last_signal[name]>20:
                events[name].append((i,d,atr));last_signal[name]=i
        hh=max(v['h'] for v in bars[i-50:i]);ll=min(v['l'] for v in bars[i-50:i])
        if c>hh and rsi>80 and abs(c-ema)>2*atr:
            add('fomo_fade_short',-1);add('fomo_breakout_long',1)
        if b['l']<ll and c>ll and b['v']>volma*2:
            add('panic_reversal_long',1)
        if all(bars[i-k]['c']>bars[i-k-1]['c'] for k in range(5)) and b['v']>volma*2 and rsi>75:
            add('herd_fade_short',-1)
        if i>2 and adxs[-1] is not None and adxs[-3] is not None:
            if adxs[-1]>40 and adxs[-1]<adxs[-2] and adxs[-2]>adxs[-3]:
                dp,dm=dis[-1]
                if dp>dm:add('chase_fade_short',-1)
                if dp<dm:add('hopeless_reversal_long',1)
    return bars,events
def replay(bars,signals,stop_atr=1.0,target_r=2.0,holding=12):
    positions={i+1:(d,atr,i) for i,d,atr in signals if i+1<len(bars)}
    active=None;trades=[]
    for i,b in enumerate(bars):
        if active:
            p=active;d=p['d'];exitprice=None;reason=None
            if b['t'].date()!=p['day'] or b['t'].hour*60+b['t'].minute>=1395:
                exitprice=bars[i-1]['c']-d*SLIPPAGE;reason='session'
            elif i>p['i']:
                if b['l']<=p['stop'] if d==1 else b['h']>=p['stop']:
                    exitprice=(min(b['o'],p['stop']) if d==1 else max(b['o'],p['stop']))-d*SLIPPAGE;reason='stop'
                elif b['h']>=p['target'] if d==1 else b['l']<=p['target']:
                    exitprice=(max(b['o'],p['target']) if d==1 else min(b['o'],p['target']))-d*SLIPPAGE;reason='target'
                elif i-p['i']>=holding:
                    exitprice=b['c']-d*SLIPPAGE;reason='timeout'
            if exitprice is not None:
                trades.append({'date':str(p['day']),'entry':p['entry'],'exit':exitprice,'net':(exitprice-p['entry'])*d-COST,'reason':reason})
                active=None
        if active is None and i in positions:
            d,atr,signal_idx=positions[i]
            if bars[signal_idx]['t'].date()!=b['t'].date() or b['t'].hour*60+b['t'].minute>=1395:continue
            entry=b['o']+d*SLIPPAGE;risk=stop_atr*atr
            if risk<=0:continue
            active={'d':d,'entry':entry,'stop':entry-d*risk,'target':entry+d*risk*target_r,'i':i,'day':b['t'].date()}
    return trades
def summarize(trades,month):
    ts=[t for t in trades if t['date'].startswith(month)]
    gp=sum(max(t['net'],0) for t in ts);gl=-sum(min(t['net'],0) for t in ts)
    daily=collections.defaultdict(float)
    for t in ts:daily[t['date']]+=t['net']
    equity=peak=dd=0.
    for _,v in sorted(daily.items()):equity+=v;peak=max(peak,equity);dd=max(dd,peak-equity)
    return dict(trades=len(ts),wins=sum(t['net']>0 for t in ts),net_points=round(sum(t['net'] for t in ts),2),pf=round(gp/gl,3) if gl else None,max_daily_close_drawdown=round(dd,2))
def main():
    for tf in (3,5):
        bars,signals=compute(tf)
        for name,ev in sorted(signals.items()):
            trades=replay(bars,ev)
            print(json.dumps({'timeframe':tf,'signal':name,'signal_count':len(ev),'september':summarize(trades,'2026-09'),'october':summarize(trades,'2026-10')}))
if __name__=='__main__':main()
