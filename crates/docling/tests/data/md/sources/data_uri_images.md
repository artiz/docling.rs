# Images in Markdown

A paragraph before the picture.

![A red box](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABgAAAAQCAIAAACDRijCAAAAHElEQVR4nGO8IyfHQA3ARBVTRg0aNWjUoBFsEABJ6QE4U1cKUgAAAABJRU5ErkJggg== "Red box")

An HTML `<img>` tag goes through the same policy:

<img src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABAAAAAYCAIAAAB8wupbAAAAHUlEQVR4nGOUs7nDQApgIkn1qIZRDaMaRjWQrAEAABgBZg/4r3kAAAAASUVORK5CYII=" alt="A blue box">

A relative path never resolves without the `local` tier: ![missing](img/not-here.png)

Trailing paragraph.
