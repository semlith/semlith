import flask


def test_make_response(app, req_ctx):
    rv = flask.make_response()
    assert rv.status_code == 200
    assert rv.data == b""
    assert rv.mimetype == "text/html"


def test_response_type_errors():
    app = flask.Flask(__name__)
    app.testing = True
