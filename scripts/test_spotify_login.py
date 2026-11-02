import unittest
from spotify_login import challenge, validate_callback


class LoginTests(unittest.TestCase):
    def test_pkce_rfc7636_vector(self):
        self.assertEqual(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM")

    def test_callback_requires_exact_state_path_and_single_code(self):
        self.assertEqual(validate_callback("/callback?state=expected&code=abc", "expected"), "abc")
        for path in ["/callback?state=wrong&code=abc", "/callback?code=abc", "/wrong?state=expected&code=abc", "/callback?state=expected&code=a&code=b", "/callback?state=expected&error=access_denied", "/callback?state=expected&state=expected&code=abc"]:
            with self.subTest(path=path), self.assertRaises(ValueError):
                validate_callback(path, "expected")


if __name__ == "__main__":
    unittest.main()
