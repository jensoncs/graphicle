"""HTTP over a private Unix socket retains authorization and safe lifecycle."""
import os
from pathlib import Path
import socket
import stat
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'sdk/python'))
from graphicle_sdk import Client, GraphicleError, LocalServer, Project

BINARY = Path(os.environ.get('GRAPHICLE_BIN', ROOT / 'crates/graphicle/target/debug/graphicle'))


class SocketURLs(unittest.TestCase):
    def test_refuses_authority_relative_path_and_ambiguous_urls(self):
        for url in ['unix://host/path', 'unix:relative', 'unix:////path',
                    'unix:///path?query', 'unix:///path#fragment', 'unix:///path%00']:
            with self.subTest(url=url), self.assertRaises(ValueError):
                Client(url, 'workspace', 'capability')


@unittest.skipUnless(os.name == 'posix' and BINARY.is_file(), 'requires Unix and a built Graphicle')
class ServeSocket(unittest.TestCase):
    def setUp(self):
        # Keep below macOS's Unix socket path limit, including the encoded-space case.
        self.tmp = tempfile.TemporaryDirectory(prefix='guds-', dir='/private/tmp' if sys.platform == 'darwin' else None)
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.path = self.root / 'review space.sock'
        self.store = self.root / 'store'

    def server(self):
        return LocalServer(BINARY, self.store, socket_path=self.path, serve_args=['--no-worker'],
                           startup_timeout=15, stop_timeout=5)

    def test_sdk_authentication_no_tcp_and_graceful_restart(self):
        server = self.server()
        with patch('socket.create_connection', side_effect=AssertionError('Unexpected TCP connection')):
            with server as client:
                self.assertTrue(server.url.startswith('unix:///'))
                self.assertIn('%20', server.url)
                self.assertTrue(stat.S_ISSOCK(self.path.stat().st_mode))
                self.assertEqual(stat.S_IMODE(self.path.stat().st_mode), 0o600)
                self.assertFalse((self.store / 'server.json').exists())
                repo = client.project('unix-review').id
                self.assertEqual(client.command('status', repository_id=repo, compact=True)['repository']['id'], repo)
                with self.assertRaises(GraphicleError) as refused:
                    Client(server.url, client.workspace, 'invalid').command('status', repository_id=repo)
                self.assertEqual(refused.exception.code, 'admission_scope')
                # A second start must not remove the first listener's path.
                inode = self.path.stat().st_ino
                with self.assertRaises(GraphicleError):
                    self.server().start()
                self.assertEqual(self.path.stat().st_ino, inode)
                client.health()
            self.assertFalse(self.path.exists())
            with self.server() as client:
                client.health()
        self.assertFalse(self.path.exists())

    def test_refuses_public_parent_and_preserves_existing_files(self):
        self.root.chmod(0o755)
        with self.assertRaisesRegex(GraphicleError, 'private directory'):
            self.server().start()
        self.assertFalse(self.path.exists())
        self.root.chmod(0o700)
        self.path.write_text('keep this file')
        with self.assertRaises(GraphicleError):
            self.server().start()
        self.assertEqual(self.path.read_text(), 'keep this file')

    def test_shutdown_does_not_remove_a_replacement_path(self):
        with self.server() as client:
            client.health()
            self.path.unlink()
            self.path.write_text('replacement')
        self.assertEqual(self.path.read_text(), 'replacement')


@unittest.skipUnless(os.name == 'posix', 'requires Unix sockets')
class TruncatedResponse(unittest.TestCase):
    """A server that dies mid-response surfaces as GraphicleError, never a raw http.client exception."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='gtr-', dir='/private/tmp' if sys.platform == 'darwin' else None)
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / 'cut.sock'
        self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.addCleanup(self.listener.close)
        self.listener.bind(str(self.path))
        self.listener.listen()
        threading.Thread(target=self.serve, daemon=True).start()

    def serve(self):
        while True:
            try:
                connection, _ = self.listener.accept()
            except OSError:
                return
            with connection:
                connection.recv(65536)
                connection.sendall(b'HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n'
                                   b'Content-Length: 100\r\n\r\n{"cut')

    def test_command_and_secondary_context_survive_a_cut_response(self):
        client = Client('unix://' + str(self.path), 'workspace', 'capability', timeout=5)
        with self.assertRaises(GraphicleError):
            client.command('status', repository_id='repo')
        result = Project(client, 'repo').secondary_context('primary', 'question', enabled=True,
                                                           expected_snapshot='0' * 64, timeout=5)
        self.assertEqual(result['secondary']['status'], 'unavailable')


if __name__ == '__main__':
    unittest.main()
