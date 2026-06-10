# Binance Alpha Network Capture - 2026-05-21

Target page:

`https://www.binance.com/zh-CN/alpha/bsc/0x365de036a1f7dccb621530d517133521debb2013`

Token context:

- Chain: `BSC`
- `chainId`: `56`
- Contract: `0x365de036a1f7dccb621530d517133521debb2013`
- Symbol: `NEX`
- Alpha asset: `ALPHA_971`
- Trading symbol: `ALPHA_971USDT`

Sensitive request headers, cookies, device ids, csrf-like tokens, and account-specific auth values are intentionally not recorded here.

## 1. Price And Market Data

### Token Full Info

```http
GET /bapi/defi/v1/public/wallet-direct/buw/wallet/cex/alpha/token/full/info?chainId=56&contractAddress=0x365de036a1f7dccb621530d517133521debb2013
```

Purpose:

- Main detail-page token metadata and price source.
- Response includes `metaInfo`, `statsInfo`, `priceInfo`.

Important response fields:

```json
{
  "data": {
    "metaInfo": {
      "tokenId": "7D0C5A6FF274499972A1145A638DDAC5",
      "name": "Nexus",
      "symbol": "NEX",
      "chainId": "56",
      "chainName": "BSC",
      "contractAddress": "0x365de036a1f7dccb621530d517133521debb2013",
      "decimals": 18,
      "alphaId": "ALPHA_971",
      "tradeDecimal": 8,
      "listingTime": 1779285600000,
      "onlineAirdrop": true,
      "trading": false
    },
    "statsInfo": {
      "volume24h": "...",
      "marketCap": "...",
      "fdv": "...",
      "liquidity": "...",
      "priceHigh24h": "...",
      "priceLow24h": "...",
      "count24h": "...",
      "holders": "..."
    },
    "priceInfo": {
      "price": "...",
      "percentChange24h": "..."
    }
  }
}
```

### Aggregate Ticker

```http
GET /bapi/defi/v1/public/alpha-trade/aggTicker24?dataType=aggregate
```

Purpose:

- Batch Alpha ticker list.
- Useful for discovery, ranking, 24h stats, and latest aggregate price.

Observed fields:

- `tokenId`
- `chainId`
- `contractAddress`
- `name`
- `symbol`
- `price`
- `percentChange24h`
- `volume24h`
- `marketCap`
- `fdv`
- `liquidity`
- `holders`
- `decimals`
- `alphaId`
- `listingTime`
- `score`
- `rwaInfo`

### Order Book Snapshot

```http
GET /bapi/defi/v1/public/alpha-trade/fullDepth?symbol=ALPHA_971USDT&limit=1000
```

Purpose:

- Full order book snapshot.
- Use for best bid/ask and strategy book bootstrap.

Response shape:

```json
{
  "data": {
    "lastUpdateId": 59174970591,
    "symbol": "ALPHA_971USDT",
    "bids": [["0.000005845", "24385377.90"]],
    "asks": [["0.000005840", "13632133.40"]],
    "E": 1779368277725,
    "T": 1779368277714
  }
}
```

### Recent Aggregate Trades

```http
GET /bapi/defi/v1/public/alpha-trade/agg-trades?limit=40&symbol=ALPHA_971USDT
```

Purpose:

- Recent public trade tape.
- Good latest traded price source.

Response item shape:

```json
{
  "a": 161336,
  "p": "0.000005854",
  "q": "93876860.60",
  "f": 161338,
  "l": 161338,
  "m": false,
  "T": 1779368241069
}
```

### Klines

```http
GET /bapi/defi/v1/public/alpha-trade/agg-klines?chainId=56&interval=15m&limit=500&tokenAddress=0x365de036a1f7dccb621530d517133521debb2013&dataType=aggregate
```

```http
GET /bapi/defi/v1/public/alpha-trade/agg-klines?chainId=56&interval=1s&limit=500&tokenAddress=0x365de036a1f7dccb621530d517133521debb2013&dataType=aggregate
```

Important notes:

- Current page uses `chainId + tokenAddress`, not only `alphaId`.
- `interval=1s` is confirmed.
- `limit=500` is optional in the first chart bootstrap request, then appears in follow-up requests.

Kline row shape:

```json
[
  "1779367591000",
  "0.0000058977543828696305",
  "0.0000058977543828696305",
  "0.000005867974520167907",
  "0.000005867974520167907",
  "10029.33748471122",
  "1779367591999"
]
```

Interpretation:

`[openTime, open, high, low, close, volume, closeTime]`

### Fee Rate

```http
GET /bapi/defi/v1/public/alpha-trade/get-fee-rate?symbol=ALPHA_971USDT
```

Observed response:

```json
{
  "data": {
    "buyerCommission": 100,
    "sellerCommission": 100
  }
}
```

### Exchange Info

```http
GET /bapi/defi/v1/public/alpha-trade/get-exchange-info
```

Purpose:

- Symbol metadata, filters, precision, min notional, lot size, order types.

Relevant `ALPHA_971USDT` shape:

```json
{
  "symbol": "ALPHA_971USDT",
  "status": "TRADING",
  "baseAsset": "ALPHA_971",
  "quoteAsset": "USDT",
  "pricePrecision": 9,
  "quantityPrecision": 2,
  "baseAssetPrecision": 2,
  "quotePrecision": 8,
  "filters": [
    {
      "filterType": "PRICE_FILTER",
      "minPrice": "0.000000001",
      "maxPrice": "1000",
      "tickSize": "0.000000001"
    },
    {
      "filterType": "LOT_SIZE",
      "stepSize": "0.10",
      "maxQty": "9999999999",
      "minQty": "0.10"
    },
    {
      "filterType": "MIN_NOTIONAL",
      "minNotional": "0.1"
    }
  ],
  "orderTypes": ["LIMIT"]
}
```

### From Assets

```http
GET /bapi/defi/v1/public/alpha-trade/get-from-asset
```

Observed response:

```json
{
  "data": ["USDT", "USDC", "U"]
}
```

## 2. Wallet, Account, And Trading Preflight

### Alpha Wallet

```http
GET /bapi/defi/v1/private/wallet-direct/cloud-wallet/alpha
```

Purpose:

- Alpha wallet holdings and valuation.

Observed response fields:

- `totalValuation`
- `list[].chainId`
- `list[].contractAddress`
- `list[].name`
- `list[].symbol`
- `list[].tokenId`
- `list[].free`
- `list[].freeze`
- `list[].amount`
- `list[].valuation`

### Terms Agreement

```http
GET /bapi/defi/v1/private/wallet-direct/swap/terms-agreement?termType=ALPHA_CEX_ORDER_DISCLAIMER
```

Observed response:

```json
{
  "data": true
}
```

### Listen Key

```http
POST /bapi/defi/v1/private/alpha-trade/get-listen-key
```

Observed response shape:

```json
{
  "data": "<listen_key>"
}
```

Do not persist the raw value in docs. Use it only at runtime.

### Average Cost

```http
POST /bapi/apex/v1/private/apex/alpha/pnl/avg-cost
```

Request body:

```json
{
  "tokens": ["ALPHA_971"]
}
```

Response shape:

```json
{
  "data": {
    "tokensAvgCost": [
      {
        "token": "ALPHA_971",
        "avgCost": "0.00000591"
      }
    ]
  }
}
```

### Wallet Asset

```http
POST /bapi/asset/v3/private/asset-service/asset/get-wallet-asset
```

Request body:

```json
{
  "includeWallets": ["CARD", "MAIN", "SAVING"],
  "includeEq": true
}
```

### Spot Wallet Balance

```http
GET /bapi/asset/v2/private/asset-service/wallet/balance?quoteAsset=USDT&needBalanceDetail=true&needEuFuture=true
```

Purpose:

- Balance panel and trading form available balance.

## 3. Orders And History

### Current Open Orders

```http
GET /bapi/defi/v1/private/alpha-trade/order/get-open-order?side=
```

Observed empty response:

```json
{
  "data": []
}
```

### Historical Limit Orders

```http
GET /bapi/defi/v1/private/alpha-trade/order/get-order-history-merge?page=1&rows=50&orderStatus=FILLED%2CPARTIALLY_FILLED%2CEXPIRED%2CCANCELED%2CREJECTED&startTime=1779206400000&endTime=1779379199000&kind=LIMIT
```

Purpose:

- Limit order history.
- Includes normal limit orders and OTO split into working/pending rows.

Observed response item shape:

```json
{
  "kind": "LIMIT",
  "orderId": "2418529",
  "symbol": "ALPHA_971USDT",
  "status": "FILLED",
  "price": "0.000005933",
  "avgPrice": "0.000005925",
  "origQty": "1685487.9",
  "executedQty": "1685487.9",
  "cumQuote": "9.9865158",
  "type": "LIMIT",
  "side": "BUY",
  "stopPrice": "0",
  "time": 1779368143285,
  "updateTime": 1779368143384,
  "orderListId": "-1",
  "baseAsset": "ALPHA_971",
  "quoteAsset": "USDT",
  "alphaId": "ALPHA_971",
  "contingencyType": null,
  "contingencyOrderPosition": null,
  "workingTime": 0,
  "targetStrategy": null
}
```

Observed OTO working row:

```json
{
  "kind": "LIMIT",
  "orderId": "2414051",
  "symbol": "ALPHA_971USDT",
  "status": "FILLED",
  "price": "0.000005929",
  "avgPrice": "0.000005929",
  "origQty": "1686625",
  "executedQty": "1686625",
  "cumQuote": "9.99999962",
  "type": "LIMIT",
  "side": "BUY",
  "orderListId": "1393062340",
  "baseAsset": "ALPHA_971",
  "quoteAsset": "USDT",
  "alphaId": "ALPHA_971",
  "contingencyType": "OTO",
  "contingencyOrderPosition": "OTO_WORKING",
  "workingTime": 1779368069148
}
```

Observed OTO pending row:

```json
{
  "kind": "LIMIT",
  "orderId": "2414052",
  "symbol": "ALPHA_971USDT",
  "status": "FILLED",
  "price": "0.000005924",
  "avgPrice": "0.000005928",
  "origQty": "1686456.3",
  "executedQty": "1686456.3",
  "cumQuote": "9.99731294",
  "type": "LIMIT",
  "side": "SELL",
  "orderListId": "1393062340",
  "baseAsset": "ALPHA_971",
  "quoteAsset": "USDT",
  "alphaId": "ALPHA_971",
  "contingencyType": "OTO",
  "contingencyOrderPosition": "OTO_PENDING",
  "workingTime": 1779368069401
}
```

Observed canceled row:

```json
{
  "kind": "LIMIT",
  "orderId": "2398360",
  "symbol": "ALPHA_971USDT",
  "status": "CANCELED",
  "price": "0.000005891",
  "avgPrice": "0",
  "origQty": "1697334.8",
  "executedQty": "0",
  "cumQuote": "0",
  "type": "LIMIT",
  "side": "SELL",
  "orderListId": "-1",
  "baseAsset": "ALPHA_971",
  "quoteAsset": "USDT",
  "alphaId": "ALPHA_971"
}
```

### User Trades

Triggered by expanding a historical order row with the left-side caret.

```http
GET /bapi/defi/v1/private/alpha-trade/order/get-user-trades?orderId=2466831&symbol=ALPHA_971USDT
```

Observed response:

```json
{
  "code": "000000",
  "message": null,
  "messageDetail": null,
  "data": [
    {
      "symbol": "ALPHA_971USDT",
      "id": "163778",
      "orderId": "2466831",
      "tradeId": "163778",
      "side": "SELL",
      "price": "0.000005619",
      "qty": "485278.90",
      "quoteQty": "2.72678213",
      "commission": "0.00027268",
      "commissionAsset": "USDT",
      "time": 1779369064032,
      "pageId": "59177352871",
      "buyer": false,
      "baseAsset": "ALPHA_971",
      "quoteAsset": "USDT",
      "orderType": "LIMIT",
      "lastTrade": true
    },
    {
      "symbol": "ALPHA_971USDT",
      "id": "163778",
      "orderId": "2466831",
      "tradeId": "163778",
      "side": "SELL",
      "price": "0.000005619",
      "qty": "1305693.70",
      "quoteQty": "7.33669290",
      "commission": "0.00073367",
      "commissionAsset": "USDT",
      "time": 1779369064032,
      "pageId": "59177352869",
      "buyer": false,
      "baseAsset": "ALPHA_971",
      "quoteAsset": "USDT",
      "orderType": "LIMIT",
      "lastTrade": false
    }
  ],
  "success": true
}
```

Notes:

- Query params are only `orderId` and `symbol`.
- One order can return multiple fills.
- `lastTrade` marks the final fill row.
- `pageId` differs per fill even when `tradeId` is the same in this observed split.

### Swap/CEX Batch Order History

```http
POST /bapi/defi/v1/private/wallet-direct/swap/cex/batch/order/history
```

Request body:

```json
{
  "page": 1,
  "pageSize": 10,
  "startTime": 1779206400000,
  "endTime": 1779379199000,
  "status": ["success", "failure"]
}
```

Purpose:

- Wallet-direct swap/CEX batch history, separate from Alpha limit order history.

Response item shape:

```json
{
  "orderId": "26051900001621468409",
  "direction": "buy",
  "fromToken": "USDT",
  "fromTokenAmount": "200.000000000000000000",
  "fromBinanceChainId": "56",
  "fromTokenId": "A97B597E63B7FCC51BB9E307E13EC2E3",
  "fromContractAddress": "0x55d398326f99059ff775485246999027b3197955",
  "toToken": "ST",
  "toTokenAmount": "4293.925838246382594135",
  "toBinanceChainId": "56",
  "toTokenId": "32820A6688D8A5301243AE230B2EDDE6",
  "toContractAddress": "0x70be40667385500c5da7f108a022e21b606045dd",
  "status": "success",
  "dbCreateTime": 1779213134000,
  "dbUpdateTime": 1779213136000
}
```

## 4. Chain And Pair Config

### Supported Alpha Chains

```http
GET /bapi/defi/v1/public/wallet-direct/buw/wallet/cex/alpha/chain/list
```

Observed chains:

- `56` BSC
- `1` Ethereum
- `CT_501` Solana
- `8453` Base
- `42161` Arbitrum
- `146` Sonic
- `CT_784` Sui
- `CT_195` TRON

### Alpha Pair Config

```http
GET /bapi/defi/v1/public/wallet-direct/buw/wallet/cex/alpha/token/pair/cfg/list
```

Observed response:

```json
{
  "data": {
    "stockAssetList": ["USDT"],
    "tokenAssetList": []
  }
}
```

## 5. Token Audit

```http
POST /bapi/defi/v1/public/wallet-direct/security/token/audit
```

Request body:

```json
{
  "binanceChainId": "56",
  "contractAddress": "0x365de036a1f7dccb621530d517133521debb2013",
  "requestId": "<uuid>"
}
```

Response includes:

- `riskLevelEnum`
- `riskLevel`
- `extraInfo.buyTax`
- `extraInfo.sellTax`
- `riskItems[]`

## 6. Still Need Live Action Capture

The following endpoints should be captured during fresh manual actions. They did not appear in the current post-login network window unless triggered before this capture started.

### Normal Limit Buy/Sell

Confirmed endpoint:

```http
POST /bapi/asset/v1/private/alpha-trade/order/place
```

Observed BUY request body:

```json
{
  "baseAsset": "ALPHA_971",
  "quoteAsset": "USDT",
  "side": "BUY",
  "price": 0.000005583,
  "quantity": 1791151.7,
  "paymentDetails": [
    {
      "amount": "9.99999994",
      "paymentWalletType": "CARD"
    }
  ],
  "orderType": "LIMIT"
}
```

Second observed BUY request body:

```json
{
  "baseAsset": "ALPHA_971",
  "quoteAsset": "USDT",
  "side": "BUY",
  "price": 0.000005578,
  "quantity": 1792757.2,
  "paymentDetails": [
    {
      "amount": "9.99999966",
      "paymentWalletType": "CARD"
    }
  ],
  "orderType": "LIMIT"
}
```

Observed response body:

```json
{
  "code": "000000",
  "message": null,
  "messageDetail": null,
  "data": "2459886",
  "success": true
}
```

Second observed response body:

```json
{
  "code": "000000",
  "message": null,
  "messageDetail": null,
  "data": "2460317",
  "success": true
}
```

Current conclusion:

- Path is unchanged.
- `paymentDetails[].amount` is still serialized as a string.
- `orderType: "LIMIT"` is still present.
- `price` and `quantity` are JSON numbers in the browser payload, but implementation should still use decimal-safe construction before serialization.
- BUY uses `paymentWalletType: "CARD"`.
- Response `data` is the order id string.

Still need to capture:

- Failed/rejected order response body if validation fails.

Observed SELL request body:

```json
{
  "baseAsset": "ALPHA_971",
  "quoteAsset": "USDT",
  "side": "SELL",
  "price": 0.000005637,
  "quantity": 1790972.6,
  "paymentDetails": [
    {
      "amount": 1790972.6,
      "paymentWalletType": "ALPHA"
    }
  ],
  "orderType": "LIMIT"
}
```

Observed SELL response body:

```json
{
  "code": "000000",
  "message": null,
  "messageDetail": null,
  "data": "2461654",
  "success": true
}
```

SELL notes:

- Path is the same as BUY.
- `side` changes to `SELL`.
- `paymentWalletType` changes to `ALPHA`.
- Unlike BUY quote amount, observed SELL `paymentDetails[].amount` is a JSON number and equals the base quantity.
- Response `data` is the order id string.

### OTO / Reverse Order

Confirmed endpoint:

```http
POST /bapi/asset/v1/private/alpha-trade/oto-order/place
```

Observed request body:

```json
{
  "baseAsset": "ALPHA_971",
  "quoteAsset": "USDT",
  "workingSide": "BUY",
  "workingPrice": 0.0000051,
  "workingQuantity": 1960784.3,
  "paymentDetails": [
    {
      "amount": "9.99999993",
      "paymentWalletType": "CARD"
    }
  ],
  "pendingPrice": 0.0000057,
  "pendingType": "LIMIT"
}
```

Second observed request body:

```json
{
  "baseAsset": "ALPHA_971",
  "quoteAsset": "USDT",
  "workingSide": "BUY",
  "workingPrice": 0.000005619,
  "workingQuantity": 1779676,
  "paymentDetails": [
    {
      "amount": "9.99999944",
      "paymentWalletType": "CARD"
    }
  ],
  "pendingPrice": 0.0000058,
  "pendingType": "LIMIT"
}
```

Observed response body:

```json
{
  "code": "000000",
  "message": null,
  "messageDetail": null,
  "data": {
    "workingOrderId": 2464786,
    "pendingOrderId": 2464787
  },
  "success": true
}
```

Second observed response body:

```json
{
  "code": "000000",
  "message": null,
  "messageDetail": null,
  "data": {
    "workingOrderId": 2465664,
    "pendingOrderId": 2465665
  },
  "success": true
}
```

OTO notes:

- Path is unchanged.
- `pendingPrice` and `pendingType` are unchanged.
- `pendingQuantity` is still omitted.
- Response returns both `workingOrderId` and `pendingOrderId` as numbers.
- For observed BUY working order, `paymentDetails[].amount` is a string quote amount and `paymentWalletType` is `CARD`.
- Canceling the observed OTO before fill used the `workingOrderId`.

### Cancel

Confirmed endpoint:

```http
POST /bapi/defi/v1/private/alpha-trade/order/cancel
```

Observed request body:

```json
{
  "orderId": "2460317",
  "symbol": "ALPHA_971USDT"
}
```

Second observed request body:

```json
{
  "orderId": "2461654",
  "symbol": "ALPHA_971USDT"
}
```

Observed response body:

```json
{
  "code": "000000",
  "message": null,
  "messageDetail": null,
  "data": null,
  "success": true
}
```

Post-cancel open-order refresh:

```http
GET /bapi/defi/v1/private/alpha-trade/order/get-open-order?side=
```

Observed post-cancel response:

```json
{
  "data": []
}
```

Cancel notes:

- Current page uses `/bapi/defi/v1/private/...`, not `/bapi/asset/v1/private/...`, for cancel.
- Body uses only `orderId` and `symbol`.
- No `orderListId` was needed for normal limit cancel.

### Cancel All

Expected endpoint:

```http
POST /bapi/defi/v1/private/alpha-trade/order/cancel-all
```

Need to capture:

- Request body.
- Response body.

### Order Detail

Expected endpoint:

```http
GET /bapi/asset/v1/private/alpha-trade/order/get-order-detail
```

Need to capture:

- Query parameters.
- Response body.

## 7. WebSocket Capture

### Token And Listen Key Setup

Market WebSocket token:

```http
GET /bapi/composite/v1/private/market/ws-token
```

Response shape:

```json
{
  "data": "<market_ws_token>"
}
```

Alpha user listen key:

```http
POST /bapi/defi/v1/private/alpha-trade/get-listen-key
```

General user stream key:

```http
POST /bapi/mbx/v1/private/mbxgateway/user-stream/start
```

Request body:

```json
{}
```

Response shape:

```json
{
  "data": "<mbx_user_stream_key>"
}
```

Do not persist raw tokens or listen keys.

### Observed WebSocket URLs

```text
wss://nbstream.binance.com/w3w/wsa/stream
wss://nbstream.binance.com/w3w/stream
wss://nbstream.binance.com/market?uuid=<uuid>&lang=zh-CN&token=<market_ws_token>&clienttype=web
wss://stream.binance.com/stream
```

The page also opened:

```text
wss://festream.saasexch.io:8443/nats-fe
```

This appears unrelated to Alpha order placement, likely a frontend/common event channel.

### Observed Subscriptions

Public Alpha market streams:

```json
{
  "method": "SUBSCRIBE",
  "params": [
    "came@allTokens@ticker24",
    "came@stockToken@metaInfo@change",
    "alpha_971usdt@fulldepth@500ms",
    "alpha_971usdt@aggTrade"
  ],
  "id": 1
}
```

Chart interval stream:

```json
{
  "method": "SUBSCRIBE",
  "params": ["came@alpha_971@short@kline_1s"],
  "id": 4
}
```

The chart later unsubscribed/resubscribed when switching intervals, for example `kline_15m` and `kline_1h`.

Alpha user stream:

```json
{
  "method": "SUBSCRIBE",
  "params": ["alpha@<alpha_listen_key>"],
  "id": 2
}
```

General user stream:

```json
{
  "method": "SUBSCRIBE",
  "params": ["<mbx_user_stream_key>"],
  "id": 5
}
```

Observed ACK shape:

```json
{
  "result": null,
  "id": 2
}
```

### Public Order Book Delta

```json
{
  "stream": "alpha_971usdt@fulldepth@500ms",
  "data": {
    "e": "depthUpdate",
    "E": 1779369537289,
    "T": 1779369537256,
    "s": "ALPHA_971USDT",
    "U": 59178703253,
    "u": 59178704340,
    "pu": 59178703146,
    "b": [["0.000005530", "34940853.20"]],
    "a": [["0.000005540", "9802441.00"]]
  }
}
```

Notes:

- `b` = bids, `a` = asks.
- Quantity `0.00` means delete that price level.
- Bootstrap with REST `fullDepth`, then apply deltas by `U/u/pu`.

### Public Aggregate Trade

```json
{
  "stream": "alpha_971usdt@aggTrade",
  "data": {
    "e": "aggTrade",
    "E": 1779369537030,
    "T": 1779369536868,
    "s": "ALPHA_971USDT",
    "a": 164916,
    "p": "0.000005540",
    "q": "98228237.90",
    "f": 164918,
    "l": 164918,
    "m": false
  }
}
```

### Public Kline

```json
{
  "stream": "came@alpha_971@short@kline_1s",
  "data": {
    "e": "kline",
    "ca": "0x365de036a1f7dccb621530d517133521debb2013@56",
    "k": {
      "i": "1s",
      "ot": 1779369468000,
      "ct": 1779369469000,
      "o": "0.000005466",
      "h": "0.000005482",
      "l": "0.000005466",
      "c": "0.00000548",
      "v": "2255.554101049"
    }
  }
}
```

### Public Ticker List

```json
{
  "stream": "came@allTokens@ticker24",
  "data": {
    "e": "tickerList",
    "d": [
      {
        "ca": "0x365de036a1f7dccb621530d517133521debb2013@56",
        "p": "0.000005526719345531",
        "pc24": "268.24",
        "vol24": "125298349.046330522979916999983",
        "mc": "331231499.15938996",
        "fdv": "552052498.59898326",
        "liq": "1806496.7257867568696384",
        "hc": "2726",
        "cnt24": 834471,
        "s": "4411",
        "t": 1779369537000
      }
    ]
  }
}
```

### User WebSocket Status

Confirmed:

- `alpha@<alpha_listen_key>` subscription is active on `wss://nbstream.binance.com/w3w/stream`.
- Subscription ACK was captured.

Still needs one fresh manual order/cancel while capture is active:

- User order event payload from `alpha@<alpha_listen_key>`.
- User trade/fill event payload if a fill occurs after subscription.

## 8. Implementation Notes

- Use `Decimal`/string-safe numeric handling for prices, quantities, and quote amounts.
- Do not use `f64` for order placement payload construction.
- Price lookup should support multiple sources:
  - token full info `priceInfo.price`
  - aggregate ticker `price`
  - order book best bid/ask
  - recent agg trades latest `p`
  - 1s klines latest close
- Historical limit orders are now available through `get-order-history-merge`, not only legacy history endpoints.
- OTO rows in history share `orderListId` and are linked by `contingencyType: "OTO"` plus `contingencyOrderPosition`.
- `get-user-trades` should be called lazily when the user expands an order detail, or proactively after a `FILLED` / `PARTIALLY_FILLED` status is observed.
