// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

package authentication

// Golden dump for the Rust port: avatars of a few users, and the first
// values of the random generator. Only runs when AKVORADO_GOLDEN_OUT is set:
//
//	AKVORADO_GOLDEN_OUT=$PWD/tests/golden/testdata/avatars.json \
//	  go test ./console/authentication -run TestGoldenDump -count=1

import (
	"context"
	"encoding/json"
	"math/rand/v2"
	"net/http"
	"net/http/httptest"
	"os"
	"testing"

	"akvorado/common/reporter"
)

func TestGoldenDump(t *testing.T) {
	out := os.Getenv("AKVORADO_GOLDEN_OUT")
	if out == "" {
		t.Skip("AKVORADO_GOLDEN_OUT not set")
	}
	c, err := New(reporter.NewMock(t), DefaultConfiguration())
	if err != nil {
		t.Fatal(err)
	}
	avatars := map[string][]byte{}
	etags := map[string]string{}
	for _, login := range []string{"__default", "alfred", "spiderman", "donald", "", "émilie", "a"} {
		req := httptest.NewRequest(http.MethodGet, "/avatar", nil)
		req = req.WithContext(context.WithValue(req.Context(), userContextKey{}, UserInformation{Login: login}))
		rec := httptest.NewRecorder()
		c.UserAvatarHandlerFunc(rec, req)
		if rec.Code != 200 {
			t.Fatalf("avatar %q: status %d", login, rec.Code)
		}
		avatars[login] = rec.Body.Bytes()
		etags[login] = rec.Header().Get("ETag")
	}
	r := rand.New(rand.NewPCG(1, 2))
	values := []uint64{r.Uint64(), r.Uint64(), r.Uint64()}
	intn := []int{}
	for _, n := range []int{3, 7, 20, 64, 1000000007} {
		intn = append(intn, r.IntN(n))
	}
	data, _ := json.MarshalIndent(map[string]any{
		"avatars": avatars, "etags": etags, "pcg": values, "intn": intn,
	}, "", " ")
	if err := os.WriteFile(out, data, 0o644); err != nil {
		t.Fatal(err)
	}
}
