# Slack alerts for manually launched Supertrend
The Rust module is apps/kite-node/src/native_node/slack_alerts.rs.
The selected LiveNode runner uses it for initialization, paused/resumed data processing and final clean/failure status. Messages label production versus paper at startup/shutdown and include the run UUID when available. Raw errors, webhook URLs, credentials and account identifiers are not included.

## Configure
The webhook pasted in chat must be replaced. Do not put the replacement in Git or a command-line argument.
Save the complete HTTPS webhook URL in Redis key susanta:slack_webhook_url.
For the local Redis instance, this Bash sequence avoids putting the value in shell history or process arguments:
```bash
read -r -s -p "New Slack webhook URL: " slack_webhook
printf '\n'
printf '%s' "$slack_webhook" | redis-cli -x SET susanta:slack_webhook_url
unset slack_webhook
```
This is a credential key; preserve it during test-data cleanup.
Use the same Redis instance as KITE_REDIS_URL if overriding the application default.

Enable alerts explicitly for a manual paper run, during trading hours:
```bash
KITE_SLACK_ALERTS=1 ./target/release/kite-node native-supertrend-session-paper config/production-supertrend.json
```
Build the new release first. Unset KITE_SLACK_ALERTS or set it to 0 to disable.
Synthetic simulations/mock commands always disable Slack, even if the environment enables it.
Enabled-but-missing/invalid configuration blocks startup; it is not silently ignored.
Actual Slack delivery has not been exercised in this change; tests use only a local HTTP server.

## Delivery behavior and limits
A dedicated worker uses certificate-verified HTTPS, JSON serialization, no redirects, a two-second connection timeout and a three-second request timeout.
Only hooks.slack.com incoming-webhook paths are accepted. Success requires HTTP 200 plus the expected ok response.
The queue is bounded to four messages. Trading never waits for HTTP; saturation/delivery failures print a local warning.
There are no automatic retries because an ambiguous timeout may mean Slack already received the message. The worker waits one second between messages.
Normal shutdown drains queued messages (at most roughly twenty seconds for an in-flight request plus a full queue). A hard kill can lose queued alerts.
The webhook is loaded once before trading, so a later Redis outage does not prevent the worker from attempting the failure notification.
Alerts are best-effort, not a durable notification queue or proof of a flat broker account.
Hard crashes, dead hosts and network outages require an independent monitor. No background service or external messages were activated.
Official API reference: https://docs.slack.dev/messaging/sending-messages-using-incoming-webhooks/
