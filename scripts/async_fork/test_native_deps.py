import hashlib
import io
import tarfile
import unittest
from unittest.mock import patch

import native_deps


class NativeDepsTests(unittest.TestCase):
    def test_download_rejects_a_checksum_mismatch(self):
        with patch.object(
            native_deps.urllib.request, "urlopen", return_value=io.BytesIO(b"changed")
        ):
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                native_deps.fetch_verified(
                    "https://example.invalid/source", hashlib.sha256(b"locked").digest()
                )

    def test_only_a_regular_locked_header_is_accepted(self):
        for regular in [True, False]:
            with self.subTest(regular=regular):
                data = io.BytesIO()
                with tarfile.open(fileobj=data, mode="w") as archive:
                    member = tarfile.TarInfo(
                        "libcap-locked/libcap/include/sys/capability.h"
                    )
                    if regular:
                        member.size = 6
                        archive.addfile(member, io.BytesIO(b"header"))
                    else:
                        member.type = tarfile.SYMTYPE
                        member.linkname = "../../outside"
                        archive.addfile(member)
                if regular:
                    self.assertEqual(
                        native_deps.capability_header(data.getvalue(), "libcap-locked"),
                        b"header",
                    )
                else:
                    with self.assertRaisesRegex(ValueError, "regular file"):
                        native_deps.capability_header(data.getvalue(), "libcap-locked")


if __name__ == "__main__":
    unittest.main()
