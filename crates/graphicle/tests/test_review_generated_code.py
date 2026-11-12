"""Generated Go code: a changed sqlc migration or proto message reaches the Go code that
calls the generated method or uses the generated type, through the real Go helper."""
import os
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'crates/graphicle/tools'))
from investigate_review_coverage import LocalServer, MOD, PROFILE, admit

BINARY = Path(os.environ.get('GRAPHICLE_BIN', ROOT / 'crates/graphicle/target/debug/graphicle'))


def files(column='int', money='int64 units = 1;'):
    return {
        'go.mod': MOD,
        'sqlc.yaml': 'version: "2"\nsql:\n  - schema: ./db/migrations/\n    queries: ./db/queries/\n'
                     '    gen:\n      go:\n        out: ./db/sqlc\n',
        'db/migrations/001_init.up.sql': f'CREATE TABLE users (id {column});\n',
        'db/queries/users.sql': '-- name: GetUser :one\nSELECT * FROM users WHERE id = $1;\n',
        'db/sqlc/users.sql.go': 'package sqlc\n\ntype Queries struct{}\n\n'
                                'func (q *Queries) GetUser(id int) int {\n\treturn id\n}\n',
        'proto/order.proto': 'syntax = "proto3";\npackage order.v1;\noption go_package = "example.test/g4/gen/order";\n\n'
                             f'message Money {{\n  {money}\n}}\n\nmessage Order {{\n  Money total = 1;\n}}\n',
        'gen/order/order.pb.go': 'package order\n\ntype Order struct {\n\tTotal int64\n}\n',
        # sqlc's emit_interface: callers reach the method through Querier, and an
        # application Store that embeds it, as the real services do.
        'db/sqlc/querier.go': 'package sqlc\n\ntype Querier interface {\n\tGetUser(id int) int\n}\n',
        'db/store/store.go': 'package store\n\nimport "example.test/g4/db/sqlc"\n\n'
                             'type Store interface {\n\tsqlc.Querier\n}\n\n'
                             'type SQLStore struct {\n\t*sqlc.Queries\n}\n\n'
                             'var DB Store = &SQLStore{Queries: &sqlc.Queries{}}\n',
        'service/service.go': 'package service\n\nimport "example.test/g4/db/store"\n\n'
                              'func Lookup() int {\n\treturn store.DB.GetUser(2)\n}\n',
        'handler/handler.go': 'package handler\n\nimport (\n\t"example.test/g4/db/sqlc"\n\t"example.test/g4/gen/order"\n)\n\n'
                              'func Handle(q *sqlc.Queries) int {\n\treturn q.GetUser(1)\n}\n\n'
                              'func Total(o *order.Order) int64 {\n\treturn o.Total\n}\n\n'
                              'func Other() int {\n\treturn 2\n}\n',
    }


@unittest.skipUnless(BINARY.is_file(), 'requires a built Graphicle')
class GeneratedCode(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='graphicle-generated-')
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.server = LocalServer(BINARY, Path(cls.tmp.name) / 'store', serve_args=['--no-worker'])
        cls.client = cls.server.__enter__()
        cls.addClassCleanup(cls.server.__exit__, None, None, None)
        cls.client.timeout = 150

    def selected(self, name, head_files, commit):
        project = self.client.project(name)
        base = admit(project, files(), 'a' * 40)
        head = admit(project, head_files, commit * 40)
        packet = project.impact([base], [head], profile=PROFILE, max_tokens=16000)
        return {e['name'] for e in packet['entries'] if e['side'] == 'head'}, packet

    def test_a_migration_reaches_the_handler_calling_its_generated_query(self):
        names, packet = self.selected('generated-sqlc', files(column='bigint'), 'b')
        self.assertIn('GetUser', names, packet['entries'])
        self.assertIn('example.test/g4/handler.Handle', names, sorted(names))
        self.assertIn('example.test/g4/service.Lookup', names, sorted(names))
        self.assertNotIn('example.test/g4/handler.Other', names)
        self.assertNotIn('example.test/g4/handler.Total', names)
        handle = next(e for e in packet['entries'] if e['name'] == 'example.test/g4/handler.Handle')
        relations = [step['relation'] for reason in handle['reasons'] for step in reason.get('steps', [])]
        self.assertIn('generated_from', relations, handle['reasons'])

    def test_a_proto_message_reaches_the_code_using_its_generated_type(self):
        names, _ = self.selected('generated-proto', files(money='int64 units = 1;\n  string currency = 2;'), 'c')
        self.assertIn('Order', names, sorted(names))
        self.assertIn('example.test/g4/handler.Total', names, sorted(names))
        self.assertNotIn('example.test/g4/handler.Handle', names)


if __name__ == '__main__':
    unittest.main()
