#!/usr/bin/python3
"""Independent, bounded graders for the opt-in delegation evaluation.

This file is never copied into worker packets. Candidates may contain one pure
Python function only. Reject I/O, imports, introspection, decorators and other
capabilities before evaluation with a minimal builtins namespace. The caller
also imposes a process deadline; a CPU limit is set here.
"""
import ast
import copy
import json
import random
import resource
import sys

FUNCTIONS = {"tags": "normalize_tags", "intervals": "merge_intervals", "checks": "summarize_checks"}
BUILTINS = {name: value for name, value in {
    "type": type, "Exception": Exception, "AttributeError": AttributeError,
    "TypeError": TypeError, "any": any, "all": all, "filter": filter,
    "len": len, "set": set, "dict": dict, "list": list, "tuple": tuple,
    "sorted": sorted, "enumerate": enumerate, "range": range, "min": min,
    "max": max, "sum": sum, "abs": abs, "isinstance": isinstance,
    "str": str, "int": int, "bool": bool, "ValueError": ValueError,
}.items()}
METHODS = {"strip", "lower", "split", "append", "add", "get", "items", "keys", "values", "copy", "sort", "fromkeys"}
NODES = {ast.Module, ast.FunctionDef, ast.arguments, ast.arg, ast.Return, ast.Assign,
         ast.AnnAssign, ast.Expr, ast.For, ast.If, ast.List, ast.Tuple, ast.Set,
         ast.Dict, ast.GeneratorExp, ast.ListComp, ast.SetComp, ast.DictComp, ast.comprehension,
         ast.Call, ast.Name, ast.Attribute, ast.Constant, ast.Compare, ast.BoolOp,
         ast.UnaryOp, ast.BinOp, ast.Subscript, ast.Slice, ast.IfExp, ast.AugAssign,
         ast.Try, ast.ExceptHandler, ast.JoinedStr, ast.FormattedValue, ast.Lambda,
         ast.keyword, ast.Raise, ast.Pass, ast.Break, ast.Continue, ast.Load, ast.Store,
         ast.Add, ast.Sub, ast.Mult, ast.Div, ast.FloorDiv, ast.Mod, ast.USub,
         ast.UAdd, ast.Not, ast.And, ast.Or, ast.Eq, ast.NotEq, ast.Lt, ast.LtE,
         ast.Gt, ast.GtE, ast.In, ast.NotIn, ast.Is, ast.IsNot}


def candidate(case, code):
    if len(code.encode()) > 16384:
        raise ValueError("candidate exceeds 16 KiB")
    tree = ast.parse(code)
    definitions = [n for n in tree.body if isinstance(n, ast.FunctionDef)]
    if len(definitions) != 1 or definitions[0].name != FUNCTIONS[case]:
        raise ValueError("public function contract changed")
    if any(not isinstance(n, ast.FunctionDef) and not (
            isinstance(n, ast.Expr) and isinstance(n.value, ast.Constant)
            and isinstance(n.value.value, str)) for n in tree.body):
        raise ValueError("unexpected top-level effects")
    nodes = list(ast.walk(tree))
    if len(nodes) > 1600:
        raise ValueError("candidate is unnecessarily large")
    for node in nodes:
        if type(node) not in NODES:
            raise ValueError("unsupported capability: " + type(node).__name__)
        if isinstance(node, ast.FunctionDef) and (node is not definitions[0] or node.decorator_list):
            raise ValueError("nested functions or decorators are outside the pure fixture")
        if isinstance(node, ast.Name) and node.id.startswith("__"):
            raise ValueError("introspection is forbidden")
        if isinstance(node, ast.Attribute) and node.attr not in METHODS:
            raise ValueError("unsupported method: " + node.attr)
        if isinstance(node, ast.Call):
            if isinstance(node.func, ast.Name) and node.func.id == "type" and (len(node.args) != 1 or node.keywords):
                raise ValueError("only one-argument type checks are allowed")
            if isinstance(node.func, ast.Name) and node.func.id not in BUILTINS:
                raise ValueError("unsupported call: " + node.func.id)
            if not isinstance(node.func, (ast.Name, ast.Attribute)):
                raise ValueError("indirect calls are forbidden")
    namespace = {"__builtins__": BUILTINS}
    exec(compile(tree, "<isolated-candidate>", "exec"), namespace)
    return namespace[FUNCTIONS[case]]


def examples(case):
    if case == "tags":
        return [("", []), (", ,", []), (" Red,blue, RED , Blue ", ["red", "blue"]),
                ("É,é,A,a", ["é", "a"]), ("A B, a b,a", ["a b", "a"]),
                ("\tX\n, Y ,x", ["x", "y"])]
    if case == "intervals":
        return [([], []), ([[4, 8]], [[4, 8]]), ([[3, 5], [1, 3]], [[1, 5]]),
                ([[1, 10], [2, 4], [10, 12]], [[1, 12]]),
                ([[8, 9], [-4, -2], [1, 3], [3, 3]], [[-4, -2], [1, 3], [8, 9]]),
                ([[0, 0], [0, 0]], [[0, 0]])]
    return [([], {"state": "unknown", "passed": 0, "failed": 0, "unknown": 0}),
            ([{"passed": True}], {"state": "passed", "passed": 1, "failed": 0, "unknown": 0}),
            ([{"passed": 1}, {"passed": "true"}, {}], {"state": "unknown", "passed": 0, "failed": 0, "unknown": 3}),
            ([{"passed": True}, {"passed": None}], {"state": "unknown", "passed": 1, "failed": 0, "unknown": 1}),
            ([{"passed": False}, {"passed": None}], {"state": "failed", "passed": 0, "failed": 1, "unknown": 1})]


def reference(case, value):
    if case == "tags":
        return list(dict.fromkeys(t.strip().lower() for t in value.split(",") if t.strip()))
    if case == "checks":
        passed = sum(c.get("passed") is True for c in value)
        failed = sum(c.get("passed") is False for c in value)
        unknown = len(value) - passed - failed
        return {"state": "failed" if failed else "unknown" if unknown or not value else "passed",
                "passed": passed, "failed": failed, "unknown": unknown}
    points = set()
    # Separate discrete half-unit coverage oracle, not the usual sort-and-fold
    # implementation given to the model. Endpoints are small fixture integers.
    for start, end in value:
        points.update(range(start * 2, end * 2 + 1))
    result = []
    for point in sorted(points):
        if result and point == result[-1][1] + 1:
            result[-1][1] = point
        else:
            result.append([point, point])
    return [[a // 2, b // 2] for a, b in result]


def grade(case, code):
    fn = candidate(case, code)
    checks = examples(case)
    randomizer = random.Random(7192026)
    for _ in range(160):
        if case == "tags":
            value = ",".join(randomizer.choices(["A", " a ", "B", "É", "é", "", "two words"], k=randomizer.randrange(15)))
        elif case == "checks":
            value = [{"passed": randomizer.choice([True, False, None, 1, 0, "false"])} for _ in range(randomizer.randrange(12))]
        else:
            value = [sorted(randomizer.choices(range(-12, 13), k=2)) for _ in range(randomizer.randrange(12))]
        checks.append((value, reference(case, value)))
    for value, expected in checks:
        argument = copy.deepcopy(value)
        actual = fn(argument)
        if actual != expected or argument != value:
            raise ValueError(f"behavior or input mutation: input={value!r}, expected={expected!r}, actual={actual!r}")
    if case == "intervals":
        try:
            fn([[4, 1]])
        except ValueError:
            pass
        else:
            raise ValueError("reversed interval must raise ValueError")
    return {"passed": True, "assertions": len(checks) * 2 + (case == "intervals")}


def self_test():
    good = {
        "tags": 'def normalize_tags(text):\n    result = []\n    for part in text.split(","):\n        tag = part.strip().lower()\n        if tag and tag not in result:\n            result.append(tag)\n    return result\n',
        "checks": 'def summarize_checks(checks):\n    passed = sum(c.get("passed") is True for c in checks)\n    failed = sum(c.get("passed") is False for c in checks)\n    unknown = len(checks) - passed - failed\n    return {"state": "failed" if failed else "unknown" if unknown or not checks else "passed", "passed": passed, "failed": failed, "unknown": unknown}\n',
    }
    good["intervals"] = 'def merge_intervals(intervals):\n    result = []\n    for start, end in sorted(intervals):\n        if start > end:\n            raise ValueError("reversed")\n        if result and start <= result[-1][1]:\n            result[-1][1] = max(end, result[-1][1])\n        else:\n            result.append([start, end])\n    return result\n'
    for case, code in good.items():
        grade(case, code)
    grade("tags", 'def normalize_tags(text):\n    return list(dict.fromkeys(filter(bool, (t.strip().lower() for t in text.split(",")))))\n')
    grade("intervals", good["intervals"].replace("sorted(intervals)", "sorted(intervals, key=lambda item: item[0])"))
    grade("intervals", good["intervals"].replace('raise ValueError("reversed")', 'raise ValueError(f"reversed {start}, {end}")'))
    grade("intervals", good["intervals"].replace("    result = []", '    if any(a > b for a, b in intervals):\n        raise ValueError("reversed")\n    result = []'))
    grade("tags", good["tags"].replace("if tag and tag not in result:", "if all([bool(tag), tag not in result]):"))
    bad = [("tags", good["tags"].replace('.lower()', '')),
           ("tags", good["tags"].replace(' and tag not in result', '')),
           ("checks", good["checks"].replace('is True', '== True')),
           ("checks", good["checks"].replace('unknown or not checks', 'unknown')),
           ("intervals", good["intervals"].replace('start <= result', 'start < result')),
           ("intervals", good["intervals"].replace('raise ValueError("reversed")', 'pass')),
           ("tags", 'import os\ndef normalize_tags(text):\n    return []'),
           ("tags", 'def normalize_tags(text):\n    return text.__class__'),
           ("tags", 'def normalize_tags(text):\n    return open("outside")')]
    grade("checks", good["checks"].replace('c.get("passed") is True', 'type(c.get("passed")) is bool and c.get("passed") is True'))
    grade("checks", good["checks"].replace('    passed =', '    try:\n        checks = checks\n    except (AttributeError, TypeError, Exception):\n        raise ValueError("unexpected")\n    passed =', 1))
    bad.extend([
        ("tags", 'def normalize_tags(text):\n    return type("X", (), {})'),
        ("tags", 'def normalize_tags(text):\n    try:\n        return open("outside")\n    except Exception:\n        return []'),
        ("intervals", good["intervals"].replace("sorted(intervals)", "sorted(intervals, key=lambda item: item.__class__)")),
        ("intervals", good["intervals"].replace("sorted(intervals)", 'sorted(intervals, key=lambda item: open("outside"))')),
        ("tags", 'def normalize_tags(text):\n    return f"{text.__class__}"'),
    ])
    for case, code in bad:
        try:
            grade(case, code)
        except (ValueError, AssertionError):
            continue
        raise AssertionError("grader accepted a known defect: " + case)
    return {"goodBaselines": len(good) + 7, "rejectedMutants": len(bad)}


if __name__ == "__main__":
    resource.setrlimit(resource.RLIMIT_CPU, (3, 3))
    try:
        if sys.argv[1:] == ["--self-test"]:
            report = self_test()
        else:
            payload = json.loads(sys.stdin.read(20000))
            report = grade(payload["case"], payload["code"])
    except Exception as error:
        report = {"passed": False, "error": str(error)}
    print(json.dumps(report))
    sys.exit(1 if report.get("passed") is False else 0)
