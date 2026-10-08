import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

import submit_indexnow as indexnow


def sitemap(urls):
    entries = ''.join(f'<url><loc>{url}</loc></url>' for url in urls)
    return f'<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">{entries}</urlset>'


class IndexNowTests(unittest.TestCase):
    def test_rejects_empty_and_noncanonical_sitemaps(self):
        cases = [[], ['https://pgsandbox.lvtd.dev/'], ['https://pgsandbox.dev.evil.example/'],
                 ['http://pgsandbox.dev/'], ['https://pgsandbox.dev/?token=private'],
                 ['https://pgsandbox.dev/#fragment']]
        for urls in cases:
            with self.assertRaises(ValueError):
                indexnow.sitemap_urls(sitemap(urls))

    def test_submission_requires_matching_live_artifacts(self):
        for mismatch in ['revision', 'key', 'sitemap']:
            with tempfile.TemporaryDirectory() as directory:
                dist = Path(directory)
                (dist / 'indexnow-key.txt').write_text('a' * 32)
                (dist / 'sitemap.xml').write_text(sitemap(['https://pgsandbox.dev/']))
                live = {'/indexnow-deploy.txt': 'sha', '/indexnow-key.txt': 'a' * 32,
                        '/sitemap.xml': sitemap(['https://pgsandbox.dev/'])}
                path = {'revision': '/indexnow-deploy.txt', 'key': '/indexnow-key.txt',
                        'sitemap': '/sitemap.xml'}[mismatch]
                live[path] = 'wrong' if mismatch != 'sitemap' else sitemap(['https://pgsandbox.dev/docs/'])
                sent = []

                def network(request, timeout):
                    if request.get_method() == 'POST':
                        sent.append(request)
                        raise AssertionError('Must not submit an unverified deployment')
                    response = MagicMock()
                    response.status = 200
                    response.url = request.full_url
                    response.read.return_value = live[request.full_url.removeprefix(indexnow.ORIGIN)].encode()
                    response.__enter__.return_value = response
                    return response

                with patch.object(indexnow, 'urlopen', side_effect=network):
                    with self.assertRaises(RuntimeError):
                        indexnow.main(['--dist', directory, '--revision', 'sha', '--attempts', '1'])
                self.assertEqual(sent, [])

    def test_verified_deployment_submits_deduplicated_urls(self):
        urls = ['https://pgsandbox.dev/', 'https://pgsandbox.dev/docs/']
        xml = sitemap(urls + urls)
        with tempfile.TemporaryDirectory() as directory:
            dist = Path(directory)
            (dist / 'indexnow-key.txt').write_text('b' * 32)
            (dist / 'sitemap.xml').write_text(xml)
            sent = []

            def network(request, timeout):
                response = MagicMock()
                response.url = request.full_url
                response.__enter__.return_value = response
                if request.get_method() == 'POST':
                    self.assertEqual(request.full_url, 'https://api.indexnow.org/indexnow')
                    sent.append(json.loads(request.data))
                    response.status = 202
                else:
                    response.status = 200
                    live = {'/indexnow-deploy.txt': 'sha', '/indexnow-key.txt': 'b' * 32, '/sitemap.xml': xml}
                    response.read.return_value = live[request.full_url.removeprefix(indexnow.ORIGIN)].encode()
                return response

            with patch.object(indexnow, 'urlopen', side_effect=network):
                indexnow.main(['--dist', directory, '--revision', 'sha', '--attempts', '1'])
            self.assertEqual(sent, [{'host': 'pgsandbox.dev', 'key': 'b' * 32,
                                    'keyLocation': 'https://pgsandbox.dev/indexnow-key.txt', 'urlList': urls}])


if __name__ == '__main__':
    unittest.main()
