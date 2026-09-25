package main

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

const testKey1 = "11111111-1111-4111-8111-111111111111"
const testKey2 = "22222222-2222-4222-8222-222222222222"
const testKey3 = "33333333-3333-4333-8333-333333333333"

func TestValidateListen(t *testing.T) {
	for _, address := range []string{"127.0.0.1:8787", "[::1]:8787", "0.0.0.0:8787", "192.168.1.10:8787"} {
		if err := validateListen(address); err != nil {
			t.Errorf("%s: %v", address, err)
		}
	}
	for _, address := range []string{"localhost:8787", ":8787", "127.0.0.1:0", "127.0.0.1:65536"} {
		if err := validateListen(address); err == nil {
			t.Errorf("%s: accepted invalid bind address", address)
		}
	}
	if defaultListen != "127.0.0.1:8787" {
		t.Fatalf("default bind must stay loopback: %s", defaultListen)
	}
}

func TestFirstRunRequiresTerminalAndKeepsCredentialsUnset(t *testing.T) {
	dir := t.TempDir()
	read, write, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	defer read.Close()
	defer write.Close()
	if _, err := initializeServer(dir, "", read, &bytes.Buffer{}); err == nil || !strings.Contains(err.Error(), "first run needs a terminal") {
		t.Fatalf("expected unattended setup instructions, got %v", err)
	}
	for _, name := range []string{"auth.json", "config.json"} {
		if _, err := os.Stat(filepath.Join(dir, name)); !os.IsNotExist(err) {
			t.Fatalf("unexpected %s after failed setup: %v", name, err)
		}
	}
}

func TestExistingPasswordSavesAndReusesListenPort(t *testing.T) {
	dir := t.TempDir()
	if err := setPassword(filepath.Join(dir, "auth.json"), strings.NewReader("a-long-test-password\n")); err != nil {
		t.Fatal(err)
	}
	read, write, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	defer read.Close()
	defer write.Close()
	for _, step := range []struct{ override, want string }{
		{"0.0.0.0:19271", "0.0.0.0:19271"},
		{"", "0.0.0.0:19271"},
	} {
		address, err := initializeServer(dir, step.override, read, &bytes.Buffer{})
		if err != nil || address != step.want {
			t.Fatalf("initialize override=%q: address=%q error=%v", step.override, address, err)
		}
	}
	config, err := readServerConfig(filepath.Join(dir, "config.json"))
	if err != nil || config.Listen != "0.0.0.0:19271" {
		t.Fatalf("saved config=%+v error=%v", config, err)
	}
	if _, err := newService(dir); err != nil {
		t.Fatalf("service cannot use initialized directory: %v", err)
	}
}

func TestInvalidExistingStateDoesNotStartSetup(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "auth.json")
	if err := os.WriteFile(path, []byte("invalid"), 0600); err != nil {
		t.Fatal(err)
	}
	read, write, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	defer read.Close()
	defer write.Close()
	if _, err := initializeServer(dir, "", read, &bytes.Buffer{}); err == nil || !strings.Contains(err.Error(), "invalid password state") {
		t.Fatalf("expected invalid password state, got %v", err)
	}
	if data, err := os.ReadFile(path); err != nil || string(data) != "invalid" {
		t.Fatalf("existing password state changed: data=%q error=%v", data, err)
	}
}

func TestPromptPort(t *testing.T) {
	for _, test := range []struct {
		input, want string
		valid       bool
	}{
		{"\n", "8787", true},
		{"49152\n", "49152", true},
		{"0\n", "", false},
		{"65536\n", "", false},
	} {
		got, err := promptPort(strings.NewReader(test.input), &bytes.Buffer{})
		if (err == nil) != test.valid || got != test.want {
			t.Errorf("port input %q: got %q, error %v", test.input, got, err)
		}
	}
}

func testService(t *testing.T) (string, *service) {
	t.Helper()
	dir := t.TempDir()
	if err := setPassword(filepath.Join(dir, "auth.json"), strings.NewReader("a-long-test-password\n")); err != nil {
		t.Fatal(err)
	}
	s, err := newService(dir)
	if err != nil {
		t.Fatal(err)
	}
	return dir, s
}

func request(t *testing.T, handler http.Handler, method, path, token string, body []byte, headers map[string]string) *httptest.ResponseRecorder {
	t.Helper()
	r := httptest.NewRequest(method, path, bytes.NewReader(body))
	if token != "" {
		r.Header.Set("Authorization", "Bearer "+token)
	}
	for name, value := range headers {
		r.Header.Set(name, value)
	}
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, r)
	return w
}

func formRequest(t *testing.T, h http.Handler, path string, values url.Values) *httptest.ResponseRecorder {
	t.Helper()
	return request(t, h, http.MethodPost, path, "", []byte(values.Encode()), map[string]string{"Content-Type": "application/x-www-form-urlencoded"})
}

func login(t *testing.T, h http.Handler) (string, string) {
	t.Helper()
	w := formRequest(t, h, "/auth/login", url.Values{
		"client_id": {clientID}, "scope": {scope}, "username": {"owner"}, "password": {"a-long-test-password"},
	})
	if w.Code != 200 {
		t.Fatalf("login status = %d: %s", w.Code, w.Body)
	}
	var tokens struct {
		Access  string `json:"access_token"`
		Refresh string `json:"refresh_token"`
		Type    string `json:"token_type"`
		Expires int    `json:"expires_in"`
		Scope   string `json:"scope"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &tokens); err != nil {
		t.Fatal(err)
	}
	if tokens.Access == "" || tokens.Refresh == "" || tokens.Type != "Bearer" || tokens.Expires < 60 || tokens.Scope != scope {
		t.Fatal("invalid token response")
	}
	return tokens.Access, tokens.Refresh
}

func TestExchangeCASReplayDeleteAndRestart(t *testing.T) {
	dir, s := testService(t)
	h := s.routes()
	unauthorized := request(t, h, "GET", "/exchange", "", nil, nil)
	if unauthorized.Code != 401 {
		t.Fatalf("unauthorized GET = %d", unauthorized.Code)
	}
	access, _ := login(t, h)
	missing := request(t, h, "GET", "/exchange", access, nil, nil)
	if missing.Code != 404 || missing.Header().Get("X-NoriShell-Next-Revision") != "1" {
		t.Fatalf("initial GET = %d, next=%s", missing.Code, missing.Header().Get("X-NoriShell-Next-Revision"))
	}
	body := []byte(`{"ciphertext":"opaque-one"}`)
	putHeaders := map[string]string{"Content-Type": contentType, "Idempotency-Key": testKey1, "X-NoriShell-Expected-Next-Revision": "1"}
	created := request(t, h, "PUT", "/exchange", access, body, putHeaders)
	if created.Code != 201 || created.Header().Get("ETag") == "" || created.Header().Get("X-NoriShell-Revision") != "1" || created.Header().Get("X-NoriShell-Updated-At-Unix-Ms") == "" {
		t.Fatalf("PUT = %d, headers=%v", created.Code, created.Header())
	}
	etag := created.Header().Get("ETag")
	replayed := request(t, h, "PUT", "/exchange", access, body, putHeaders)
	if replayed.Code != 201 || replayed.Header().Get("ETag") != etag {
		t.Fatalf("PUT replay = %d", replayed.Code)
	}
	changedReplay := request(t, h, "PUT", "/exchange", access, []byte("changed"), putHeaders)
	if changedReplay.Code != 409 {
		t.Fatalf("changed replay = %d", changedReplay.Code)
	}
	read := request(t, h, "GET", "/exchange", access, nil, nil)
	if read.Code != 200 || !bytes.Equal(read.Body.Bytes(), body) || read.Header().Get("ETag") != etag {
		t.Fatalf("GET after PUT = %d", read.Code)
	}
	stale := request(t, h, "PUT", "/exchange", access, body, map[string]string{"Content-Type": contentType, "Idempotency-Key": testKey2, "If-Match": `"stale"`})
	if stale.Code != 412 {
		t.Fatalf("stale PUT = %d", stale.Code)
	}
	deleteHeaders := map[string]string{"If-Match": etag, "Idempotency-Key": testKey2}
	deleted := request(t, h, "DELETE", "/exchange", access, nil, deleteHeaders)
	if deleted.Code != 204 || deleted.Body.Len() != 0 {
		t.Fatalf("DELETE = %d, body=%s", deleted.Code, deleted.Body)
	}
	if w := request(t, h, "DELETE", "/exchange", access, nil, deleteHeaders); w.Code != 204 {
		t.Fatalf("DELETE replay = %d", w.Code)
	}
	missing = request(t, h, "GET", "/exchange", access, nil, nil)
	if missing.Code != 404 || missing.Header().Get("X-NoriShell-Next-Revision") != "3" {
		t.Fatalf("GET after DELETE = %d, next=%s", missing.Code, missing.Header().Get("X-NoriShell-Next-Revision"))
	}
	restarted, err := newService(dir)
	if err != nil {
		t.Fatal(err)
	}
	h = restarted.routes()
	if w := request(t, h, "DELETE", "/exchange", access, nil, deleteHeaders); w.Code != 204 {
		t.Fatalf("DELETE replay after restart = %d", w.Code)
	}
	recreated := request(t, h, "PUT", "/exchange", access, []byte(`{"ciphertext":"opaque-two"}`), map[string]string{"Content-Type": contentType, "Idempotency-Key": testKey3, "X-NoriShell-Expected-Next-Revision": "3"})
	if recreated.Code != 201 || recreated.Header().Get("X-NoriShell-Revision") != "3" {
		t.Fatalf("PUT after restart = %d", recreated.Code)
	}
	if _, err := os.Stat(filepath.Join(dir, "exchange.json")); err != nil {
		t.Fatal(err)
	}
}

func TestOverwriteRequiresCurrentStrongETag(t *testing.T) {
	_, s := testService(t)
	h := s.routes()
	access, _ := login(t, h)
	first := request(t, h, "PUT", "/exchange", access, []byte("one"), map[string]string{"Content-Type": contentType, "Idempotency-Key": testKey1, "X-NoriShell-Expected-Next-Revision": "1"})
	if first.Code != 201 {
		t.Fatalf("first PUT = %d", first.Code)
	}
	second := request(t, h, "PUT", "/exchange", access, []byte("two"), map[string]string{"Content-Type": contentType, "Idempotency-Key": testKey2, "If-Match": first.Header().Get("ETag")})
	if second.Code != 200 || second.Header().Get("X-NoriShell-Revision") != "2" || second.Header().Get("ETag") == first.Header().Get("ETag") {
		t.Fatalf("overwrite = %d, headers=%v", second.Code, second.Header())
	}
	if w := request(t, h, "DELETE", "/exchange", access, nil, map[string]string{"Idempotency-Key": testKey3, "If-Match": first.Header().Get("ETag")}); w.Code != 412 {
		t.Fatalf("stale DELETE = %d", w.Code)
	}
}

func TestConcurrentInitialWritesHaveOneWinner(t *testing.T) {
	_, service := testService(t)
	handler := service.routes()
	access, _ := login(t, handler)
	start := make(chan struct{})
	codes := make(chan int, 2)
	var writers sync.WaitGroup
	for index, key := range []string{testKey1, testKey2} {
		writers.Add(1)
		go func(index int, key string) {
			defer writers.Done()
			<-start
			request := httptest.NewRequest(http.MethodPut, "/exchange", strings.NewReader(string(rune('a'+index))))
			request.Header.Set("Authorization", "Bearer "+access)
			request.Header.Set("Content-Type", contentType)
			request.Header.Set("Idempotency-Key", key)
			request.Header.Set("X-NoriShell-Expected-Next-Revision", "1")
			response := httptest.NewRecorder()
			handler.ServeHTTP(response, request)
			codes <- response.Code
		}(index, key)
	}
	close(start)
	writers.Wait()
	close(codes)
	wins, stale := 0, 0
	for code := range codes {
		switch code {
		case http.StatusCreated:
			wins++
		case http.StatusPreconditionFailed:
			stale++
		default:
			t.Fatalf("concurrent PUT = %d", code)
		}
	}
	if wins != 1 || stale != 1 {
		t.Fatalf("concurrent writes: created=%d stale=%d", wins, stale)
	}
	read := request(t, handler, http.MethodGet, "/exchange", access, nil, nil)
	if read.Code != http.StatusOK || read.Header().Get("X-NoriShell-Revision") != "1" || (read.Body.String() != "a" && read.Body.String() != "b") {
		t.Fatalf("unexpected winner: code=%d revision=%q body=%q", read.Code, read.Header().Get("X-NoriShell-Revision"), read.Body.String())
	}
}

func TestRefreshRevokeAndPasswordRotation(t *testing.T) {
	dir, s := testService(t)
	h := s.routes()
	access, refresh := login(t, h)
	if formRequest(t, h, "/auth/token", url.Values{"client_id": {clientID}, "grant_type": {"refresh_token"}}).Code != 401 {
		t.Fatal("empty refresh token issued a session")
	}
	w := formRequest(t, h, "/auth/token", url.Values{"client_id": {clientID}, "grant_type": {"refresh_token"}, "refresh_token": {refresh}})
	if w.Code != 200 {
		t.Fatalf("refresh = %d", w.Code)
	}
	var next struct {
		Access  string `json:"access_token"`
		Refresh string `json:"refresh_token"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &next); err != nil {
		t.Fatal(err)
	}
	if request(t, h, "GET", "/exchange", access, nil, nil).Code != 401 || request(t, h, "GET", "/exchange", next.Access, nil, nil).Code != 404 {
		t.Fatal("refresh did not rotate access token")
	}
	if formRequest(t, h, "/auth/token", url.Values{"client_id": {clientID}, "grant_type": {"refresh_token"}, "refresh_token": {refresh}}).Code != 401 {
		t.Fatal("consumed refresh token still works")
	}
	if formRequest(t, h, "/auth/revoke", url.Values{"client_id": {clientID}, "token": {next.Refresh}, "token_type_hint": {"refresh_token"}}).Code != 200 {
		t.Fatal("revoke failed")
	}
	if request(t, h, "GET", "/exchange", next.Access, nil, nil).Code != 401 {
		t.Fatal("revoked access token still works")
	}
	access, _ = login(t, h)
	if err := setPassword(filepath.Join(dir, "auth.json"), strings.NewReader("a-new-long-password\n")); err != nil {
		t.Fatal(err)
	}
	if request(t, h, "GET", "/exchange", access, nil, nil).Code != 401 {
		t.Fatal("password rotation did not invalidate session")
	}
}

func TestRejectOversizeAndUnsafeConditions(t *testing.T) {
	_, s := testService(t)
	h := s.routes()
	access, _ := login(t, h)
	oversize := request(t, h, "PUT", "/exchange", access, bytes.Repeat([]byte("x"), maxExchangeBytes+1), map[string]string{"Content-Type": contentType, "Idempotency-Key": testKey1, "X-NoriShell-Expected-Next-Revision": "1"})
	if oversize.Code != 413 {
		t.Fatalf("oversize = %d", oversize.Code)
	}
	for _, condition := range []string{"0", "01", "9007199254740992"} {
		w := request(t, h, "PUT", "/exchange", access, []byte("x"), map[string]string{"Content-Type": contentType, "Idempotency-Key": testKey1, "X-NoriShell-Expected-Next-Revision": condition})
		if w.Code != 400 {
			t.Fatalf("bad next revision %q = %d", condition, w.Code)
		}
	}
	if request(t, h, "GET", "/exchange?token=secret", access, nil, nil).Code != 400 {
		t.Fatal("query string accepted")
	}
}
