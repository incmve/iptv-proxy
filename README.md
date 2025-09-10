# Iptv Proxy

[![Actions Status](https://github.com/pierre-emmanuelJ/iptv-proxy/workflows/CI/badge.svg)](https://github.com/pierre-emmanuelJ/iptv-proxy/actions?query=workflow%3ACI)

NOTE: This fork of the [original project](https://github.com/pierre-emmanuelJ/iptv-proxy) contains the following enhancements:

- Corrected issue with Xtream Codes EPG not loading
- Fixed issue with Xtream Codes VOD (Shows & Movies) as the IPTV provider returned data seems to be partially complete, or missing pieces that this proxy is expecting
- Continue on error EXTINF
- Added Gluetun for VPN

## Description 

Iptv-Proxy is a project to proxyfie an m3u file
and to proxyfie an Xtream iptv service (client API).

### M3U and M3U8

M3U service convert an iptv m3u file into a web proxy server.

It's transform all the original tracks to an new url pointing on the proxy.


### Xtream code client api

proxy on Xtream code (client API)

support live, vod, series and full epg :rocket:

### M3u Example

Original iptv m3u file

```m3u
#EXTM3U
#EXTINF:-1 tvg-ID="examplechanel1.com" tvg-name="chanel1" tvg-logo="http://ch.xyz/logo1.png" group-title="USA HD",CHANEL1-HD
http://iptvexample.net:1234/12/test/1
#EXTINF:-1 tvg-ID="examplechanel2.com" tvg-name="chanel2" tvg-logo="http://ch.xyz/logo2.png" group-title="USA HD",CHANEL2-HD
http://iptvexample.net:1234/13/test/2
#EXTINF:-1 tvg-ID="examplechanel3.com" tvg-name="chanel3" tvg-logo="http://ch.xyz/logo3.png" group-title="USA HD",CHANEL3-HD
http://iptvexample.net:1234/14/test/3
#EXTINF:-1 tvg-ID="examplechanel4.com" tvg-name="chanel4" tvg-logo="http://ch.xyz/logo4.png" group-title="USA HD",CHANEL4-HD
http://iptvexample.net:1234/15/test/4
```

What M3U proxy IPTV do
 - convert chanels url to new endpoints
 - convert original m3u file with new routes pointing to the proxy

Start proxy server example

```Bash
iptv-proxy --m3u-url http://example.com/get.php?username=user&password=pass&type=m3u_plus&output=m3u8 \
             --port 8080 \
             --hostname proxyexample.com \
             --user test \
             --password passwordtest
```


 That's give you an m3u file on a specific endpoint `iptv.m3u` in our example
 
 `http://proxyserver.com:8080/iptv.m3u?username=test&password=passwordtest`

All the new routes pointing on your proxy server
```m3u
#EXTM3U
#EXTINF:-1 tvg-ID="examplechanel1.com" tvg-name="chanel1" tvg-logo="http://ch.xyz/logo1.png" group-title="USA HD",CHANEL1-HD
http://proxyserver.com:8080/12/test/1?username=test&password=passwordtest
#EXTINF:-1 tvg-ID="examplechanel2.com" tvg-name="chanel2" tvg-logo="http://ch.xyz/logo2.png" group-title="USA HD",CHANEL2-HD
http://proxyserver.com:8080/13/test/2?username=test&password=passwordtest
#EXTINF:-1 tvg-ID="examplechanel3.com" tvg-name="chanel3" tvg-logo="http://ch.xyz/logo3.png" group-title="USA HD",CHANEL3-HD
http://proxyserver.com:8080/14/test/3?username=test&password=passwordtest
#EXTINF:-1 tvg-ID="examplechanel4.com" tvg-name="chanel4" tvg-logo="http://ch.xyz/logo4.png" group-title="USA HD",CHANEL4-HD
http://proxyserver.com:8080/15/test/4?username=test&password=passwordtest
```

### M3u8 Example

The m3u8 feature is like m3u.
The playlist should be in the m3u format and should contain all m3u8 tracks.

Sample of the original m3u file containing m3u8 track:
```Shell
#EXTM3U
#EXTINF:-1 tvg-ID="examplechanel1.com" tvg-name="chanel1" tvg-logo="http://ch.xyz/logo1.png" group-title="USA HD",CHANEL1-HD
http://iptvexample.net:1234/12/test/1.m3u8
#EXTINF:-1 tvg-ID="examplechanel2.com" tvg-name="chanel2" tvg-logo="http://ch.xyz/logo2.png" group-title="USA HD",CHANEL2-HD
http://iptvexample.net:1234/13/test/2.m3u8
```

### Xtream code client API example

```Bash
% iptv-proxy --m3u-url http://example.com:1234/get.php?username=user&password=pass&type=m3u_plus&output=m3u8 \
             --port 8080 \
             --hostname proxyexample.com \
             ## put xtream flags if you want to add xtream proxy
             --xtream-user xtream_user \
             --xtream-password xtream_password \
             --xtream-base-url http://example.com:1234 \
             --user test \
             --password passwordtest
             
```

What Xtream proxy do

 - convert xtream `xtream-user ` and `xtream-password` into new `user` and `password`
 - convert `xtream-base-url` with `hostname` and `port`
 
Original xtream credentials
 
 ```
 user: xtream_user
 password: xtream_password
 base-url: http://example.com:1234
 ```
 
New xtream credentials

 ```
 user: test
 password: passwordtest
 base-url: http://proxyexample.com:8080
 ```
 
 All xtream live, streams, vod, series... are proxyfied! 
 
 
 You can get the m3u file with the original Xtream api request:
 ```
 http://proxyexample.com:8080/get.php?username=test&password=passwordtest&type=m3u_plus&output=ts
 ```


## Installation
## With Docker

### Prerequisite

 - Add an m3u URL in `docker-compose.yml` or add local file in `iptv` folder
 - `HOSTNAME` and `PORT` to expose
 - CHange VPN credentials and provider

```Yaml
 environment:
      # if you are using m3u remote file
      # M3U_URL: http://example.com:1234/get.php?username=user&password=pass&type=m3u_plus&output=m3u8
      M3U_URL: /root/iptv/iptv.m3u
      # Hostname or IP to expose the IPTVs endpoints (for machine not for docker)
      HOSTNAME: localhost
      GIN_MODE: release
      ## Xtream-code proxy configuration
      ## (put these env variables if you want to add xtream proxy)
      XTREAM_USER: xtream_user
      XTREAM_PASSWORD: xtream_password
      XTREAM_BASE_URL: "http://example.com:1234"
      USER: test
      PASSWORD: testpassword
```

### Start

```
% docker-compose up -d
```

## TODO

there is basic auth just for testing.
change with a real auth with database and user management
and auth with token...

**ENJOY!**

## Powered by

- [cobra](https://github.com/spf13/cobra)
- [go.xtream-codes](https://github.com/tellytv/go.xtream-codes)
- [gin](https://github.com/gin-gonic/gin)

Grab me a beer 🍻

[![paypal](https://www.paypalobjects.com/en_US/i/btn/btn_donate_LG.gif)](https://www.paypal.com/donate?hosted_button_id=WQAAMQWJPKHUN)

