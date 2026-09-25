package main

import (
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"html/template"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	webCookieName     = "norishell_web_session"
	webCSRFCookieName = "norishell_web_csrf"
	webLifetime       = 8 * time.Hour
	maxWebSessions    = 128
)

type webSession struct {
	epoch   string
	expires time.Time
}

type webSessions struct {
	mu     sync.Mutex
	tokens map[string]webSession
}

var webPage = template.Must(template.New("web").Parse(`<!doctype html>
<html lang="{{.Text.Language}}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{.Text.Title}}</title><style>
body{font:16px/1.6 system-ui,sans-serif;max-width:580px;margin:8vh auto;padding:0 20px;color:#1d2939;background:#f8fafc}
main{background:white;border:1px solid #e2e8f0;border-radius:12px;padding:28px;box-shadow:0 4px 20px #0f172a0a}
h1{font-size:24px;margin:0 0 12px}p{color:#475467}label{display:block;margin:18px 0 6px}
input{box-sizing:border-box;width:100%;padding:10px;font:inherit;border:1px solid #98a2b3;border-radius:6px}
button{padding:9px 18px;margin-top:18px;font:inherit;color:white;background:#1d4ed8;border:0;border-radius:6px;cursor:pointer}
dl{display:grid;grid-template-columns:minmax(130px,1fr) 2fr;gap:10px}dt{color:#475467}dd{margin:0;overflow-wrap:anywhere}
.error{color:#b42318}
</style></head><body><main><h1>{{.Text.Title}}</h1>
{{if .LoggedIn}}
<p>{{.Text.MetadataNote}}</p>
<dl><dt>{{.Text.CloudData}}</dt><dd>{{if .HasData}}{{.Text.Stored}}{{else}}{{.Text.None}}{{end}}</dd>
<dt>{{.Text.ServerVersion}}</dt><dd>{{.ServerVersion}}</dd>
<dt>{{.Text.CurrentRevision}}</dt><dd>{{.Revision}}</dd>
<dt>{{.Text.NextRevision}}</dt><dd>{{.NextRevision}}</dd>
<dt>{{.Text.EncryptedSize}}</dt><dd>{{.Bytes}} {{.Text.Bytes}}</dd>
<dt>{{.Text.LastWrite}}</dt><dd>{{.Updated}}</dd></dl>
<form method="post" action="/web/logout"><input type="hidden" name="csrf_token" value="{{.CSRFToken}}"><button type="submit">{{.Text.Logout}}</button></form>
{{else}}
<p>{{.Text.LoginNote}}</p>
{{if .InsecureHTTP}}<p role="note">{{.Text.HTTPWarning}}</p>{{end}}
{{if .Error}}<p class="error" role="alert">{{.Text.LoginError}}</p>{{end}}
<form method="post" action="/web/login"><input type="hidden" name="csrf_token" value="{{.CSRFToken}}"><label for="password">{{.Text.Password}}</label>
<input id="password" name="password" type="password" minlength="12" maxlength="1024" autocomplete="current-password" required autofocus>
<button type="submit">{{.Text.Login}}</button></form>
{{end}}</main></body></html>`))

type webText struct {
	Language        string
	Title           string
	MetadataNote    string
	CloudData       string
	ServerVersion   string
	Stored          string
	None            string
	CurrentRevision string
	NextRevision    string
	EncryptedSize   string
	Bytes           string
	LastWrite       string
	Logout          string
	LoginNote       string
	HTTPWarning     string
	LoginError      string
	Password        string
	Login           string
}

var webChinese = webText{
	Language: "zh-CN", Title: "NoriShell 同步服务",
	MetadataNote: "这里只显示服务端保存的同步元信息。同步数据已加密，网页无法读取 Host、凭据或密钥。",
	CloudData:    "云端数据", ServerVersion: "服务端版本", Stored: "已存储", None: "暂无", CurrentRevision: "当前 revision",
	NextRevision: "下一 revision", EncryptedSize: "密文包大小", Bytes: "字节", LastWrite: "最近写入",
	Logout: "退出登录", LoginNote: "使用同步服务密码登录，查看云端存储状态。",
	HTTPWarning: "当前使用 HTTP，密码会以明文传输。请只在可信本地网络使用；公网请使用 HTTPS。",
	LoginError:  "密码错误或登录暂不可用。", Password: "同步服务密码", Login: "登录",
}

var webEnglish = webText{
	Language: "en", Title: "NoriShell Sync Server",
	MetadataNote: "This page shows only sync metadata stored by the server. The sync data is encrypted; this page cannot read hosts, credentials, or keys.",
	CloudData:    "Cloud data", ServerVersion: "Server version", Stored: "Stored", None: "None", CurrentRevision: "Current revision",
	NextRevision: "Next revision", EncryptedSize: "Encrypted package size", Bytes: "bytes", LastWrite: "Last write",
	Logout: "Log out", LoginNote: "Sign in with your sync server password to view cloud storage status.",
	HTTPWarning: "This connection uses HTTP, so the password is sent in plaintext. Use it only on a trusted local network; use HTTPS on the public Internet.",
	LoginError:  "Incorrect password or sign-in is temporarily unavailable.", Password: "Sync server password", Login: "Sign in",
}

func webLanguage(r *http.Request) string {
	header := r.Header.Get("Accept-Language")
	if len(header) > 512 {
		return "zh-CN"
	}
	bestQuality := -1.0
	selected := "zh-CN"
	for _, entry := range strings.Split(header, ",") {
		parts := strings.Split(entry, ";")
		tag := strings.ToLower(strings.TrimSpace(parts[0]))
		language := ""
		switch {
		case tag == "zh" || strings.HasPrefix(tag, "zh-"):
			language = "zh-CN"
		case tag == "en" || strings.HasPrefix(tag, "en-"):
			language = "en"
		default:
			continue
		}
		quality := 1.0
		for _, parameter := range parts[1:] {
			key, value, ok := strings.Cut(strings.TrimSpace(parameter), "=")
			if !ok || !strings.EqualFold(strings.TrimSpace(key), "q") {
				continue
			}
			parsed, err := strconv.ParseFloat(strings.TrimSpace(value), 64)
			if err != nil || parsed < 0 || parsed > 1 {
				quality = 0
			} else {
				quality = parsed
			}
			break
		}
		if quality > 0 && quality > bestQuality {
			bestQuality = quality
			selected = language
		}
	}
	return selected
}

func webTextFor(r *http.Request) webText {
	if webLanguage(r) == "en" {
		return webEnglish
	}
	return webChinese
}

type webView struct {
	LoggedIn      bool
	Error         bool
	InsecureHTTP  bool
	HasData       bool
	ServerVersion string
	Revision      uint64
	NextRevision  uint64
	Bytes         int
	Updated       string
	CSRFToken     string
	Text          webText
}

func webHeaders(w http.ResponseWriter) {
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("Referrer-Policy", "no-referrer")
	w.Header().Set("X-Frame-Options", "DENY")
	w.Header().Set("Content-Security-Policy", "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'")
	w.Header().Add("Vary", "Cookie")
	w.Header().Add("Vary", "Accept-Language")
}

func webError(w http.ResponseWriter, r *http.Request, status int, zh, en string) {
	text := zh
	if webLanguage(r) == "en" {
		text = en
	}
	w.Header().Set("Content-Language", webLanguage(r))
	w.Header().Add("Vary", "Accept-Language")
	http.Error(w, text, status)
}

func newWebCSRFKey() ([32]byte, error) {
	var key [32]byte
	_, err := rand.Read(key[:])
	return key, err
}

func validWebCSRFNonce(value string) bool {
	if len(value) != 43 {
		return false
	}
	decoded, err := base64.RawURLEncoding.DecodeString(value)
	return err == nil && len(decoded) == 32 && base64.RawURLEncoding.EncodeToString(decoded) == value
}

func uniqueWebCookie(r *http.Request, name string) (*http.Cookie, bool) {
	var found *http.Cookie
	for _, cookie := range r.Cookies() {
		if cookie.Name != name {
			continue
		}
		if found != nil {
			return nil, false
		}
		found = cookie
	}
	return found, found != nil
}

func (s *service) webCSRFToken(nonce, action, session string) string {
	mac := hmac.New(sha256.New, s.webCSRFKey[:])
	_, _ = mac.Write([]byte(action))
	_, _ = mac.Write([]byte{0})
	_, _ = mac.Write([]byte(nonce))
	_, _ = mac.Write([]byte{0})
	_, _ = mac.Write([]byte(session))
	return base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
}

func (s *service) writeWebPage(w http.ResponseWriter, r *http.Request, status int, view webView) {
	nonce := ""
	if cookie, ok := uniqueWebCookie(r, webCSRFCookieName); ok && validWebCSRFNonce(cookie.Value) {
		nonce = cookie.Value
	} else {
		var err error
		nonce, err = randomString(32)
		if err != nil {
			webError(w, r, http.StatusServiceUnavailable, "页面暂不可用", "Page unavailable")
			return
		}
	}
	action := "login"
	session := ""
	if view.LoggedIn {
		action = "logout"
		cookie, ok := uniqueWebCookie(r, webCookieName)
		if !ok || len(cookie.Value) != 43 {
			webError(w, r, http.StatusServiceUnavailable, "页面暂不可用", "Page unavailable")
			return
		}
		session = cookie.Value
	}
	view.CSRFToken = s.webCSRFToken(nonce, action, session)
	view.Text = webTextFor(r)
	view.ServerVersion = serverVersion
	webBrowserCookie(w, r, webCSRFCookieName, nonce, int(webLifetime.Seconds()))
	view.InsecureHTTP = r.TLS == nil && !strings.EqualFold(r.Header.Get("X-Forwarded-Proto"), "https")
	webHeaders(w)
	w.Header().Set("Content-Language", view.Text.Language)
	w.WriteHeader(status)
	_ = webPage.Execute(w, view)
}

func (s *service) validWebCSRF(r *http.Request, action string) bool {
	values := r.PostForm["csrf_token"]
	if len(values) != 1 || len(values[0]) != 43 {
		return false
	}
	cookie, ok := uniqueWebCookie(r, webCSRFCookieName)
	if !ok || !validWebCSRFNonce(cookie.Value) {
		return false
	}
	session := ""
	if action == "logout" {
		webCookie, ok := uniqueWebCookie(r, webCookieName)
		if !ok || len(webCookie.Value) != 43 {
			return false
		}
		session = webCookie.Value
	}
	expected := s.webCSRFToken(cookie.Value, action, session)
	return subtle.ConstantTimeCompare([]byte(values[0]), []byte(expected)) == 1
}

func webBrowserCookie(w http.ResponseWriter, r *http.Request, name, value string, maxAge int) {
	secure := r.TLS != nil || strings.EqualFold(r.Header.Get("X-Forwarded-Proto"), "https")
	http.SetCookie(w, &http.Cookie{Name: name, Value: value, Path: "/", HttpOnly: true, Secure: secure, SameSite: http.SameSiteStrictMode, MaxAge: maxAge})
}

func webCookie(w http.ResponseWriter, r *http.Request, value string, maxAge int) {
	webBrowserCookie(w, r, webCookieName, value, maxAge)
}

func (s *service) webIndex(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path != "/" {
		webError(w, r, http.StatusNotFound, "页面不存在", "Page not found")
		return
	}
	if r.Method != http.MethodGet && r.Method != http.MethodHead {
		w.Header().Set("Allow", "GET, HEAD")
		webError(w, r, http.StatusMethodNotAllowed, "不支持此请求方式", "Method not allowed")
		return
	}
	state, err := readPassword(s.passwordPath)
	if err != nil {
		webError(w, r, http.StatusServiceUnavailable, "认证暂不可用", "Authentication unavailable")
		return
	}
	if !s.webAuthenticated(r, state.Epoch) {
		s.writeWebPage(w, r, http.StatusOK, webView{})
		return
	}
	exchange, poisoned := s.exchange.get()
	if poisoned {
		webError(w, r, http.StatusServiceUnavailable, "存储暂不可用", "Storage unavailable")
		return
	}
	view := webView{LoggedIn: true, HasData: exchange.Revision != 0, Revision: exchange.Revision, NextRevision: exchange.NextRevision, Bytes: len(exchange.Body), Updated: webTextFor(r).None}
	if exchange.UpdatedMS > 0 {
		view.Updated = time.UnixMilli(exchange.UpdatedMS).UTC().Format("2006-01-02 15:04:05 UTC")
	}
	s.writeWebPage(w, r, http.StatusOK, view)
}

func (s *service) webAuthenticated(r *http.Request, epoch string) bool {
	cookie, ok := uniqueWebCookie(r, webCookieName)
	if !ok || len(cookie.Value) != 43 {
		return false
	}
	s.web.mu.Lock()
	defer s.web.mu.Unlock()
	session, ok := s.web.tokens[tokenHash(cookie.Value)]
	return ok && session.expires.After(time.Now()) && subtle.ConstantTimeCompare([]byte(session.epoch), []byte(epoch)) == 1
}

func (s *service) webLogin(w http.ResponseWriter, r *http.Request) {
	if !readForm(w, r) {
		return
	}
	if !s.validWebCSRF(r, "login") {
		webError(w, r, http.StatusForbidden, "表单已失效，请刷新页面后重试", "Form expired. Refresh the page and try again")
		return
	}
	if len(r.PostForm["password"]) != 1 {
		webError(w, r, http.StatusBadRequest, "表单无效", "Invalid form")
		return
	}
	select {
	case s.auth.attempts <- struct{}{}:
		defer func() { <-s.auth.attempts }()
	default:
		webError(w, r, http.StatusTooManyRequests, "登录请求繁忙，请稍后重试", "Sign-in is busy. Try again shortly")
		return
	}
	if !s.auth.reservePasswordAttempt() {
		w.Header().Set("Retry-After", "60")
		webError(w, r, http.StatusTooManyRequests, "登录尝试过多，请稍后重试", "Too many sign-in attempts. Try again later")
		return
	}
	state, err := readPassword(s.passwordPath)
	if err != nil {
		webError(w, r, http.StatusServiceUnavailable, "认证暂不可用", "Authentication unavailable")
		return
	}
	if !checkPassword(state, r.PostForm.Get("password")) {
		s.writeWebPage(w, r, http.StatusUnauthorized, webView{Error: true})
		return
	}
	token, err := randomString(32)
	if err != nil {
		webError(w, r, http.StatusServiceUnavailable, "认证暂不可用", "Authentication unavailable")
		return
	}
	s.web.mu.Lock()
	now := time.Now()
	for hash, item := range s.web.tokens {
		if !item.expires.After(now) || item.epoch != state.Epoch {
			delete(s.web.tokens, hash)
		}
	}
	if len(s.web.tokens) >= maxWebSessions {
		s.web.mu.Unlock()
		webError(w, r, http.StatusServiceUnavailable, "登录会话过多", "Too many sign-in sessions")
		return
	}
	s.web.tokens[tokenHash(token)] = webSession{epoch: state.Epoch, expires: now.Add(webLifetime)}
	s.web.mu.Unlock()
	webCookie(w, r, token, int(webLifetime.Seconds()))
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("Location", "/")
	w.WriteHeader(http.StatusSeeOther)
}

func (s *service) webLogout(w http.ResponseWriter, r *http.Request) {
	if !readForm(w, r) {
		return
	}
	if !s.validWebCSRF(r, "logout") {
		webError(w, r, http.StatusForbidden, "表单已失效，请刷新页面后重试", "Form expired. Refresh the page and try again")
		return
	}
	if cookie, ok := uniqueWebCookie(r, webCookieName); ok && len(cookie.Value) == 43 {
		s.web.mu.Lock()
		delete(s.web.tokens, tokenHash(cookie.Value))
		s.web.mu.Unlock()
	}
	webCookie(w, r, "", -1)
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("Location", "/")
	w.WriteHeader(http.StatusSeeOther)
}
