-- One persistent key per account scope, shared by all workers/instruments.
local raw=redis.call('GET',KEYS[1])
if not raw or #raw>150000 or redis.call('PTTL',KEYS[1])~=-1 then return redis.error_reply('invalid limiter state') end
local s=cjson.decode(raw)
if s.version~=1 or s.policy~=ARGV[1] or type(s.history)~='table' or #s.history>5000 or type(s.last_ms)~='number' or type(s.cooldown_until)~='number' then return redis.error_reply('invalid limiter schema') end
local t=redis.call('TIME')
local now=tonumber(t[1])*1000+math.floor(tonumber(t[2])/1000)
if now<s.last_ms then return redis.error_reply('clock moved backwards') end
local previous=0
local history={}
local second={}
local minute={}
for _,stamp in ipairs(s.history) do
    if type(stamp)~='number' or stamp<previous or stamp>s.last_ms then return redis.error_reply('invalid limiter history') end
    previous=stamp
    if stamp>now-86400000 then table.insert(history,stamp) end
    if stamp>now-60000 then table.insert(minute,stamp) end
    if stamp>now-1000 then table.insert(second,stamp) end
end
if ARGV[2]=='cooldown' then
    s.cooldown_until=math.max(s.cooldown_until,now+tonumber(ARGV[3]))
    s.history=history
    s.last_ms=now
    redis.call('SET',KEYS[1],cjson.encode(s))
    return {0,s.cooldown_until-now}
end
local p=cjson.decode(ARGV[1])
local retry=math.max(0,s.cooldown_until-now)
if #second>=p.per_second then retry=math.max(retry,second[#second-p.per_second+1]+1000-now) end
if #minute>=p.per_minute then retry=math.max(retry,minute[#minute-p.per_minute+1]+60000-now) end
if #history>=p.per_day then retry=math.max(retry,history[#history-p.per_day+1]+86400000-now) end
if retry>0 then return {0,retry} end
table.insert(history,now)
s.history=history
s.last_ms=now
redis.call('SET',KEYS[1],cjson.encode(s))
return {1,0}
