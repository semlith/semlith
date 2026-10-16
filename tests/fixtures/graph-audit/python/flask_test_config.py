import flask


def test_custom_config_class():
    class Config(flask.Config):
        pass

    class Flask(flask.Flask):
        config_class = Config

    app = Flask(__name__)
    assert isinstance(app.config, Config)


def test_config_from_object():
    app = flask.Flask(__name__)
    app.config.from_object(__name__)
