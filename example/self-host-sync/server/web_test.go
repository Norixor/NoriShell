package main

import (
	"net/http"
	"net/http/httptest"
	"net/url"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"
)

var csrfInputPattern = regexp.MustCompile(`name="csrf_token" value="([^"]+)"`)

func webPageToken(t *testing.T, page *httptest.ResponseRecorder) (*http.Cookie, string) {
	t.Helper()
	match := csrfInputPattern.FindStringSubmatch(page.Body.String())
	if len(match) != 2 {
		t.Fatal("CSRF form token missing")
	}
	for _, cookie := range page.Result().Cookies() {
		if cookie.Name == webCSRFCookieName {
			if !cookie.HttpOnly || cookie.SameSite != http.SameSiteStrictMode || cookie.MaxAge <= 0 || !validWebCSRFNonce(cookie.Value) {
				t.Fatalf("unsafe CSRF cookie: %+v", cookie)
			}
			return cookie, match[1]
		}
	}
	t.Fatal("CSRF cookie missing")
	return nil, ""
}

func webForm(t *testing.T, h http.Handler, path string, values url.Values, cookie *http.Cookie, origin string) *httptest.ResponseRecorder {
	t.Helper()
	csrfCookie, token := webPageToken(t, webGet(t, h, cookie))
	form := url.Values{}
	for key, entries := range values {
		form[key] = append([]string(nil), entries...)
	}
	form.Set("csrf_token", token)
	r := httptest.NewRequest(http.MethodPost, path, strings.NewReader(form.Encode()))
	r.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	if origin != "" {
		r.Header.Set("Origin", origin)
	}
	if cookie != nil {
		r.AddCookie(cookie)
	}
	r.AddCookie(csrfCookie)
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	return w
}

func webGet(t *testing.T, h http.Handler, cookie *http.Cookie) *httptest.ResponseRecorder {
	t.Helper()
	r := httptest.NewRequest(http.MethodGet, "/", nil)
	if cookie != nil {
		r.AddCookie(cookie)
	}
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	return w
}

func webTestLogin(t *testing.T, h http.Handler) *http.Cookie {
	t.Helper()
	w := webForm(t, h, "/web/login", url.Values{"password": {"a-long-test-password"}}, nil, "http://example.com")
	if w.Code != http.StatusSeeOther || w.Header().Get("Location") != "/" {
		t.Fatalf("web login status=%d", w.Code)
	}
	for _, cookie := range w.Result().Cookies() {
		if cookie.Name == webCookieName {
			if !cookie.HttpOnly || cookie.SameSite != http.SameSiteStrictMode || cookie.MaxAge <= 0 || len(cookie.Value) != 43 {
				t.Fatalf("unsafe web cookie: %+v", cookie)
			}
			return cookie
		}
	}
	t.Fatal("web cookie missing")
	return nil
}

func TestWebBilingualPagesAndErrors(t *testing.T) {
	for _, test := range []struct {
		accept string
		want   string
	}{
		{"", "zh-CN"},
		{"zh-CN,zh;q=0.9,en;q=0.8", "zh-CN"},
		{"en-US,en;q=0.9,zh;q=0.8", "en"},
		{"en;q=0.2,zh;q=0.9", "zh-CN"},
		{"en;q=0,zh;q=0.5", "zh-CN"},
		{"fr-FR,en;q=0.9", "en"},
	} {
		r := httptest.NewRequest(http.MethodGet, "/", nil)
		r.Header.Set("Accept-Language", test.accept)
		if got := webLanguage(r); got != test.want {
			t.Errorf("Accept-Language %q: got %q, want %q", test.accept, got, test.want)
		}
	}
	_, s := testService(t)
	h := s.routes()
	pageRequest := httptest.NewRequest(http.MethodGet, "/", nil)
	pageRequest.Header.Set("Accept-Language", "en-US,en;q=0.9,zh;q=0.8")
	page := httptest.NewRecorder()
	h.ServeHTTP(page, pageRequest)
	if page.Code != http.StatusOK || page.Header().Get("Content-Language") != "en" || !strings.Contains(strings.Join(page.Header().Values("Vary"), ","), "Accept-Language") || !strings.Contains(page.Body.String(), `<html lang="en">`) || !strings.Contains(page.Body.String(), "Sync server password") || !strings.Contains(page.Body.String(), "password is sent in plaintext") || strings.Contains(page.Body.String(), "同步服务密码") {
		t.Fatalf("English login page incorrect: status=%d body=%s", page.Code, page.Body.String())
	}
	csrfCookie, csrfToken := webPageToken(t, page)
	post := func(password, token string) *httptest.ResponseRecorder {
		t.Helper()
		form := url.Values{"password": {password}, "csrf_token": {token}}
		r := httptest.NewRequest(http.MethodPost, "/web/login", strings.NewReader(form.Encode()))
		r.Header.Set("Content-Type", "application/x-www-form-urlencoded")
		r.Header.Set("Accept-Language", "en-US")
		r.AddCookie(csrfCookie)
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		return w
	}
	if stale := post("a-long-test-password", "invalid"); stale.Code != http.StatusForbidden || !strings.Contains(stale.Body.String(), "Refresh the page") || stale.Header().Get("Content-Language") != "en" {
		t.Fatalf("English form error incorrect: %d %s", stale.Code, stale.Body.String())
	}
	if wrong := post("wrong", csrfToken); wrong.Code != http.StatusUnauthorized || !strings.Contains(wrong.Body.String(), "Incorrect password") || !strings.Contains(wrong.Body.String(), `<html lang="en">`) {
		t.Fatalf("English password error incorrect: %d %s", wrong.Code, wrong.Body.String())
	}
	login := post("a-long-test-password", csrfToken)
	if login.Code != http.StatusSeeOther {
		t.Fatalf("English login status=%d", login.Code)
	}
	var session *http.Cookie
	for _, cookie := range login.Result().Cookies() {
		if cookie.Name == webCookieName {
			session = cookie
		}
	}
	if session == nil {
		t.Fatal("web session missing")
	}
	statusRequest := httptest.NewRequest(http.MethodGet, "/", nil)
	statusRequest.Header.Set("Accept-Language", "en-US")
	statusRequest.AddCookie(session)
	status := httptest.NewRecorder()
	h.ServeHTTP(status, statusRequest)
	if status.Code != http.StatusOK || !strings.Contains(status.Body.String(), "Server version</dt><dd>"+serverVersion) || !strings.Contains(status.Body.String(), "Current revision") || !strings.Contains(status.Body.String(), "Last write</dt><dd>None") || !strings.Contains(status.Body.String(), "Log out") || strings.Contains(status.Body.String(), "最近写入") {
		t.Fatalf("English status page incorrect: %d %s", status.Code, status.Body.String())
	}
}

func TestWebStatusUsesStoredMetadataOnly(t *testing.T) {
	_, s := testService(t)
	h := s.routes()
	guest := webGet(t, h, nil)
	if guest.Code != 200 || !strings.Contains(guest.Body.String(), "同步服务密码") || !strings.Contains(guest.Body.String(), "HTTP，密码会以明文传输") || strings.Contains(guest.Body.String(), "下一 revision") {
		t.Fatalf("guest page status=%d", guest.Code)
	}
	if guest.Header().Get("Cache-Control") != "no-store" || guest.Header().Get("Content-Security-Policy") == "" {
		t.Fatal("web protection headers missing")
	}
	cookie := webTestLogin(t, h)
	empty := webGet(t, h, cookie)
	if !strings.Contains(empty.Body.String(), "暂无") || !strings.Contains(empty.Body.String(), "下一 revision</dt><dd>1") {
		t.Fatalf("empty status: %s", empty.Body.String())
	}
	access, _ := login(t, h)
	secret := "opaque-encrypted-exchange-private-marker"
	created := request(t, h, http.MethodPut, "/exchange", access, []byte(secret), map[string]string{"Content-Type": contentType, "Idempotency-Key": testKey1, "X-NoriShell-Expected-Next-Revision": "1"})
	if created.Code != http.StatusCreated {
		t.Fatalf("PUT status=%d", created.Code)
	}
	page := webGet(t, h, cookie)
	if page.Code != 200 || !strings.Contains(page.Body.String(), "已存储") || !strings.Contains(page.Body.String(), "当前 revision</dt><dd>1") || !strings.Contains(page.Body.String(), "密文包大小</dt><dd>40 字节") || !strings.Contains(page.Body.String(), "UTC") || strings.Contains(page.Body.String(), secret) {
		t.Fatalf("stored status omitted or leaked exchange: %s", page.Body.String())
	}
	if w := request(t, h, http.MethodGet, "/exchange", "", nil, map[string]string{"Cookie": webCookieName + "=" + cookie.Value}); w.Code != http.StatusUnauthorized {
		t.Fatalf("web cookie granted API access: %d", w.Code)
	}
}

func TestWebLogoutRotationAndCSRF(t *testing.T) {
	dir, s := testService(t)
	h := s.routes()
	badLogin := httptest.NewRequest(http.MethodPost, "/web/login", strings.NewReader(url.Values{"password": {"a-long-test-password"}}.Encode()))
	badLogin.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	badLogin.Header.Set("Origin", "https://evil.example")
	badResponse := httptest.NewRecorder()
	h.ServeHTTP(badResponse, badLogin)
	if badResponse.Code != http.StatusForbidden {
		t.Fatalf("login without page token=%d", badResponse.Code)
	}
	cookie := webTestLogin(t, h)
	badLogout := httptest.NewRequest(http.MethodPost, "/web/logout", strings.NewReader(""))
	badLogout.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	badLogout.Header.Set("Origin", "https://evil.example")
	badLogout.AddCookie(cookie)
	badResponse = httptest.NewRecorder()
	h.ServeHTTP(badResponse, badLogout)
	if badResponse.Code != http.StatusForbidden {
		t.Fatalf("logout without page token=%d", badResponse.Code)
	}
	if w := webGet(t, h, cookie); !strings.Contains(w.Body.String(), "下一 revision") {
		t.Fatal("invalid logout revoked session")
	}
	if w := webForm(t, h, "/web/logout", nil, cookie, "http://example.com"); w.Code != http.StatusSeeOther {
		t.Fatalf("logout=%d", w.Code)
	}
	if w := webGet(t, h, cookie); strings.Contains(w.Body.String(), "下一 revision") {
		t.Fatal("logout failed")
	}
	cookie = webTestLogin(t, h)
	if err := setPassword(filepath.Join(dir, "auth.json"), strings.NewReader("a-new-long-password\n")); err != nil {
		t.Fatal(err)
	}
	if w := webGet(t, h, cookie); strings.Contains(w.Body.String(), "下一 revision") {
		t.Fatal("password change did not invalidate web session")
	}
	if w := request(t, h, http.MethodGet, "/?password=leak", "", nil, nil); w.Code != http.StatusBadRequest {
		t.Fatalf("query rule changed: %d", w.Code)
	}
}

func TestWebLoginBoundedAndNoOtherRoutes(t *testing.T) {
	_, s := testService(t)
	h := s.routes()
	for i := 0; i < 10; i++ {
		w := webForm(t, h, "/web/login", url.Values{"password": {"wrong"}}, nil, "http://example.com")
		if w.Code != http.StatusUnauthorized {
			t.Fatalf("wrong login %d=%d", i, w.Code)
		}
	}
	if w := webForm(t, h, "/web/login", url.Values{"password": {"a-long-test-password"}}, nil, "http://example.com"); w.Code != http.StatusTooManyRequests {
		t.Fatalf("login limit=%d", w.Code)
	}
	if w := request(t, h, http.MethodGet, "/some/other/path", "", nil, nil); w.Code != http.StatusNotFound {
		t.Fatalf("unexpected route=%d", w.Code)
	}
}

func TestWebLoginBehindHTTPSProxySetsSecureCookie(t *testing.T) {
	_, s := testService(t)
	h := s.routes()
	pageRequest := httptest.NewRequest(http.MethodGet, "http://127.0.0.1/", nil)
	pageRequest.Host = "127.0.0.1"
	pageRequest.Header.Set("X-Forwarded-Proto", "https")
	page := httptest.NewRecorder()
	h.ServeHTTP(page, pageRequest)
	csrfCookie, csrfToken := webPageToken(t, page)
	if !csrfCookie.Secure || strings.Contains(page.Body.String(), "HTTP，密码会以明文传输") {
		t.Fatal("HTTPS proxy page or CSRF cookie treated as HTTP")
	}
	form := url.Values{"password": {"a-long-test-password"}, "csrf_token": {csrfToken}}.Encode()
	r := httptest.NewRequest(http.MethodPost, "http://127.0.0.1/web/login", strings.NewReader(form))
	r.Host = "127.0.0.1"
	r.Header.Set("Origin", "https://sync.example.test:8443")
	r.Header.Set("X-Forwarded-Proto", "https")
	r.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	r.AddCookie(csrfCookie)
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	cookies := w.Result().Cookies()
	if w.Code != http.StatusSeeOther || len(cookies) != 1 || !cookies[0].Secure {
		t.Fatalf("HTTPS proxy login status=%d, cookie count=%d", w.Code, len(cookies))
	}
}

func TestWebCSRFRejectsForgedDuplicateAndCrossSessionTokens(t *testing.T) {
	dir, s := testService(t)
	h := s.routes()
	csrfCookie, loginToken := webPageToken(t, webGet(t, h, nil))
	post := func(path string, form url.Values, cookies ...*http.Cookie) int {
		t.Helper()
		r := httptest.NewRequest(http.MethodPost, path, strings.NewReader(form.Encode()))
		r.Header.Set("Content-Type", "application/x-www-form-urlencoded")
		for _, cookie := range cookies {
			r.AddCookie(cookie)
		}
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		return w.Code
	}
	password := "a-long-test-password"
	if code := post("/web/login", url.Values{"password": {password}, "csrf_token": {"forged"}}, csrfCookie); code != http.StatusForbidden {
		t.Fatalf("forged token=%d", code)
	}
	if code := post("/web/login", url.Values{"password": {password}, "csrf_token": {loginToken, loginToken}}, csrfCookie); code != http.StatusForbidden {
		t.Fatalf("duplicate token fields=%d", code)
	}
	if code := post("/web/login", url.Values{"password": {password}, "csrf_token": {loginToken}}, csrfCookie, csrfCookie); code != http.StatusForbidden {
		t.Fatalf("duplicate CSRF cookies=%d", code)
	}
	firstSession := webTestLogin(t, h)
	firstCSRF, logoutToken := webPageToken(t, webGet(t, h, firstSession))
	secondSession := webTestLogin(t, h)
	attackerCSRF, attackerToken := webPageToken(t, webGet(t, h, secondSession))
	if code := post("/web/logout", url.Values{"csrf_token": {loginToken}}, firstSession, firstCSRF); code != http.StatusForbidden {
		t.Fatalf("login token used for logout=%d", code)
	}
	if code := post("/web/logout", url.Values{"csrf_token": {logoutToken}}, secondSession, firstCSRF); code != http.StatusForbidden {
		t.Fatalf("token used with another session=%d", code)
	}
	if code := post("/web/logout", url.Values{"csrf_token": {attackerToken}}, firstSession, attackerCSRF); code != http.StatusForbidden {
		t.Fatalf("attacker CSRF cookie used with victim session=%d", code)
	}
	if code := post("/web/logout", url.Values{"csrf_token": {attackerToken}}, firstSession, firstCSRF, attackerCSRF); code != http.StatusForbidden {
		t.Fatalf("duplicate sibling CSRF cookie=%d", code)
	}
	if code := post("/web/logout", url.Values{"csrf_token": {logoutToken}}, firstSession, secondSession, firstCSRF); code != http.StatusForbidden {
		t.Fatalf("duplicate session cookie=%d", code)
	}
	if page := webGet(t, h, firstSession); !strings.Contains(page.Body.String(), "下一 revision") {
		t.Fatal("invalid CSRF request revoked victim session")
	}
	if code := post("/web/logout", url.Values{"csrf_token": {logoutToken}}, firstSession, firstCSRF); code != http.StatusSeeOther {
		t.Fatalf("valid logout=%d", code)
	}
	restarted, err := newService(dir)
	if err != nil {
		t.Fatal(err)
	}
	h = restarted.routes()
	if code := post("/web/login", url.Values{"password": {password}, "csrf_token": {loginToken}}, csrfCookie); code != http.StatusForbidden {
		t.Fatalf("stale form token=%d", code)
	}
}

func TestWebAndAPILoginShareAttemptLimit(t *testing.T) {
	_, s := testService(t)
	h := s.routes()
	for i := 0; i < 5; i++ {
		if w := webForm(t, h, "/web/login", url.Values{"password": {"wrong"}}, nil, "http://example.com"); w.Code != http.StatusUnauthorized {
			t.Fatalf("web attempt %d=%d", i, w.Code)
		}
		if w := formRequest(t, h, "/auth/login", url.Values{"client_id": {clientID}, "scope": {scope}, "username": {"owner"}, "password": {"wrong"}}); w.Code != http.StatusUnauthorized {
			t.Fatalf("API attempt %d=%d", i, w.Code)
		}
	}
	if w := webForm(t, h, "/web/login", url.Values{"password": {"a-long-test-password"}}, nil, "http://example.com"); w.Code != http.StatusTooManyRequests {
		t.Fatalf("web did not respect shared limit: %d", w.Code)
	}
	if w := formRequest(t, h, "/auth/login", url.Values{"client_id": {clientID}, "scope": {scope}, "username": {"owner"}, "password": {"a-long-test-password"}}); w.Code != http.StatusTooManyRequests {
		t.Fatalf("API did not respect shared limit: %d", w.Code)
	}
	s.auth.mu.Lock()
	s.auth.passwordAttempts = []time.Time{time.Now().Add(-2 * time.Minute)}
	s.auth.mu.Unlock()
	if w := formRequest(t, h, "/auth/login", url.Values{"client_id": {clientID}, "scope": {scope}, "username": {"owner"}, "password": {"a-long-test-password"}}); w.Code != http.StatusOK {
		t.Fatalf("API did not recover after time window: %d", w.Code)
	}
}
