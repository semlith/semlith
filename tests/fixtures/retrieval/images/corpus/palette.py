"""Colour names, so a third file answers a colour query without being a picture.

Corpus only. The point of the image class is that a query about a picture
reaches the picture; a file that lists every colour word in the question set is
the strongest text competitor that can be written, and it belongs in the corpus
for that reason rather than in spite of it.
"""

NAMED = {
    "red": (220, 38, 38),
    "blue": (37, 99, 235),
    "green": (22, 163, 74),
    "yellow": (250, 204, 21),
    "purple": (147, 51, 234),
    "orange": (234, 88, 12),
    "black": (17, 17, 17),
    "white": (255, 255, 255),
}

# What each word tends to be drawn as, which is the association a text model
# has and an image model does not need.
ASSOCIATIONS = {
    "red": "a circle, a stop sign, a warning",
    "blue": "a square, the sky, a link",
    "green": "a triangle, a check mark, a field",
    "yellow": "horizontal stripes, a highlighter, a road marking",
    "purple": "a diagonal line, a gradient, a bruise",
    "orange": "a ring, a donut, a traffic cone",
    "grey": "a gradient fading from dark to light",
}


def rgb(name):
    """Look a colour up by name."""
    return NAMED[name.lower()]


def describe(name):
    """What that colour is usually a picture of."""
    return ASSOCIATIONS.get(name.lower(), "nothing in particular")
