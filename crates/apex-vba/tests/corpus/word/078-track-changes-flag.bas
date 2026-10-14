' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: True
' corpus-prints: False
Sub Main()
    ActiveDocument.TrackRevisions = True
    Debug.Print ActiveDocument.TrackRevisions
    ActiveDocument.TrackRevisions = False
    Debug.Print ActiveDocument.TrackRevisions
End Sub
